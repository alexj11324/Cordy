import { act, cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { I18nProvider } from "@patchbay/core/i18n/react";
import { dingtalkKeys } from "@patchbay/core/dingtalk";
import { toast } from "sonner";
import enCommon from "../../locales/en/common.json";
import enSettings from "../../locales/en/settings.json";

const mocks = vi.hoisted(() => ({
  listMembers: vi.fn(), listAgents: vi.fn(), listDingTalkInstallations: vi.fn(),
  listDingTalkGroups: vi.fn(), listDingTalkGroupRoutes: vi.fn(), updateDingTalkGroupRoute: vi.fn(),
}));
vi.mock("@patchbay/core/api", () => ({ api: mocks }));
vi.mock("@patchbay/core/hooks", () => ({ useWorkspaceId: () => "workspace-1" }));
vi.mock("@patchbay/core/auth", () => ({
  useAuthStore: Object.assign(
    (select: (state: { user: { id: string } }) => unknown) => select({ user: { id: "user-1" } }),
    { getState: () => ({ user: { id: "user-1" } }) },
  ),
}));
vi.mock("@patchbay/core/workspace/hooks", () => ({
  useActorName: () => ({ getAgentName: () => "Default agent" }),
}));
vi.mock("../../common/actor-avatar", () => ({ ActorAvatar: () => null }));
vi.mock("../../platform", () => ({ openExternal: vi.fn() }));
vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn() } }));

import { DingTalkTab } from "./dingtalk-tab";

const originalRoute = {
  id: "route-1", workspace_id: "workspace-1", installation_id: "bot-1",
  conversation_id: "cid-release", conversation_title: "Release team", agent_id: "agent-1",
  discovered_at: "", updated_at: "",
};
let currentRoute = originalRoute;
const clients: QueryClient[] = [];

beforeEach(() => {
  vi.resetAllMocks();
  currentRoute = { ...originalRoute };
  mocks.listMembers.mockResolvedValue([{ user_id: "user-1", role: "owner" }]);
  mocks.listAgents.mockResolvedValue([
    { id: "agent-1", name: "Default agent", archived_at: null },
    { id: "agent-2", name: "Release agent", archived_at: null },
    { id: "archived", name: "Archived agent", archived_at: "2026-01-01" },
    { id: "patrick", name: "Patrick", system_key: "patrick", archived_at: null },
  ]);
  mocks.listDingTalkInstallations.mockResolvedValue({
    configured: true, group_routing_supported: true,
    installations: [{ id: "bot-1", agent_id: "agent-1", status: "active", installed_at: "" }],
  });
  mocks.listDingTalkGroups.mockResolvedValue({ groups: [], group_discovery_supported: false });
  mocks.listDingTalkGroupRoutes.mockImplementation(async () => ({ routes: [currentRoute] }));
  mocks.updateDingTalkGroupRoute.mockImplementation(async (_ws, _id, body) => {
    currentRoute = { ...currentRoute, agent_id: body.agent_id };
    return currentRoute;
  });
});

afterEach(() => {
  cleanup();
  clients.splice(0).forEach((client) => client.clear());
});

function renderSettings() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  clients.push(client);
  render(<QueryClientProvider client={client}>
    <I18nProvider locale="en" resources={{ en: { common: enCommon, settings: enSettings } }}>
      <DingTalkTab />
    </I18nProvider>
  </QueryClientProvider>);
  return client;
}

async function selectReleaseAgent() {
  await userEvent.click(await screen.findByRole("combobox", { name: "Agent for Release team" }));
  await userEvent.click(await screen.findByRole("option", { name: "Release agent" }));
}

describe("DingTalk group routing in Settings", () => {
  it("changes the selection only after the server accepts the reassignment", async () => {
    let accept!: (value: typeof originalRoute) => void;
    mocks.updateDingTalkGroupRoute.mockImplementationOnce(() => new Promise((resolve) => { accept = resolve; }));
    const client = renderSettings();
    await selectReleaseAgent();
    expect(mocks.updateDingTalkGroupRoute).toHaveBeenCalledWith("workspace-1", "route-1", { agent_id: "agent-2" });
    const selector = screen.getByRole("combobox", { name: "Agent for Release team" });
    expect(selector).toBeDisabled();
    expect(selector).toHaveTextContent("Default agent");
    currentRoute = { ...originalRoute, agent_id: "agent-2" };
    await act(async () => accept(currentRoute));
    await waitFor(() => expect(selector).toHaveTextContent("Release agent"));
    expect(client.getQueryData(dingtalkKeys.groupRoutes("workspace-1"))).toEqual({ routes: [currentRoute] });
    expect(toast.success).toHaveBeenCalled();
  });

  it("keeps the previous assignment after failure and lets the admin retry", async () => {
    mocks.updateDingTalkGroupRoute.mockRejectedValueOnce(new Error("temporarily unavailable"));
    renderSettings();
    await selectReleaseAgent();
    await waitFor(() => expect(toast.error).toHaveBeenCalled());
    expect(screen.getByRole("combobox", { name: "Agent for Release team" })).toHaveTextContent("Default agent");
    await selectReleaseAgent();
    await waitFor(() => expect(screen.getByRole("combobox", { name: "Agent for Release team" })).toHaveTextContent("Release agent"));
    expect(mocks.updateDingTalkGroupRoute).toHaveBeenCalledTimes(2);
  });

  it("does not announce success for a malformed update response", async () => {
    mocks.updateDingTalkGroupRoute.mockResolvedValueOnce({ id: "", agent_id: "" });
    renderSettings();
    await selectReleaseAgent();
    await waitFor(() => expect(toast.error).toHaveBeenCalled());
    expect(toast.success).not.toHaveBeenCalled();
    expect(screen.getByRole("combobox", { name: "Agent for Release team" })).toHaveTextContent("Default agent");
  });

  it("shows a route-loading error with retry, not a false empty state", async () => {
    mocks.listDingTalkGroupRoutes.mockRejectedValueOnce(new Error("offline"));
    renderSettings();
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("Could not load group routes");
    expect(screen.queryByText("No groups discovered yet")).not.toBeInTheDocument();
    await userEvent.click(within(alert).getByRole("button", { name: "Retry" }));
    expect(await screen.findByRole("combobox", { name: "Agent for Release team" })).toBeEnabled();
  });

  it("leaves assignments visible but disables editing when the agent list fails", async () => {
    mocks.listAgents.mockRejectedValue(new Error("agent lookup failed"));
    renderSettings();
    const selector = await screen.findByRole("combobox", { name: "Agent for Release team" });
    expect(selector).toBeDisabled();
    expect(await screen.findByText("Could not load Agents")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Retry Agents" })).toBeEnabled();
  });

  it("allows members to read group assignments without mutation controls", async () => {
    mocks.listMembers.mockResolvedValue([{ user_id: "user-1", role: "member" }]);
    renderSettings();
    expect(await screen.findByRole("heading", { name: "Group routing" })).toBeInTheDocument();
    expect(await screen.findByText("Release team")).toBeInTheDocument();
    expect(screen.queryByRole("combobox")).not.toBeInTheDocument();
    expect(mocks.updateDingTalkGroupRoute).not.toHaveBeenCalled();
  });

  it("excludes archived agents but keeps product-defined user agents eligible", async () => {
    renderSettings();
    await userEvent.click(await screen.findByRole("combobox", { name: "Agent for Release team" }));
    expect(await screen.findByRole("option", { name: "Release agent" })).toBeInTheDocument();
    expect(screen.queryByRole("option", { name: "Archived agent" })).not.toBeInTheDocument();
    expect(screen.getByRole("option", { name: "Patrick" })).toBeInTheDocument();
  });

  it("does not query routes when the backend does not support them", async () => {
    mocks.listDingTalkInstallations.mockResolvedValue({
      configured: true, installations: [{ id: "bot-1", agent_id: "agent-1", status: "active", installed_at: "" }],
    });
    renderSettings();
    await screen.findByText("Default agent");
    expect(screen.queryByRole("heading", { name: "Group routing" })).not.toBeInTheDocument();
    expect(mocks.listDingTalkGroupRoutes).not.toHaveBeenCalled();
  });

  it("shows discovery instructions when a supported bot has no group routes", async () => {
    mocks.listDingTalkGroupRoutes.mockResolvedValue({ routes: [] });
    renderSettings();
    expect(await screen.findByText("No groups discovered yet")).toBeInTheDocument();
    expect(screen.queryByRole("combobox")).not.toBeInTheDocument();
  });

  it("only displays routes for the current workspace and visible active installations", async () => {
    mocks.listDingTalkGroupRoutes.mockResolvedValue({ routes: [
      originalRoute,
      { ...originalRoute, id: "other-workspace", workspace_id: "workspace-2", conversation_title: "Other workspace group" },
      { ...originalRoute, id: "other-bot", installation_id: "bot-2", conversation_title: "Other bot group" },
    ] });
    renderSettings();
    await screen.findByRole("combobox", { name: "Agent for Release team" });
    expect(screen.queryByText("Other workspace group")).not.toBeInTheDocument();
    expect(screen.queryByText("Other bot group")).not.toBeInTheDocument();
  });
});
