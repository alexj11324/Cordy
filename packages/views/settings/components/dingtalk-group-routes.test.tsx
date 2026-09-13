import { act, cleanup, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { dingtalkKeys } from "@orvilo/core/dingtalk";
import { toast } from "sonner";
import { renderWithI18n } from "../../test/i18n";

const mocks = vi.hoisted(() => ({
  listMembers: vi.fn(), listAgents: vi.fn(), listDingTalkInstallations: vi.fn(),
  listDingTalkGroups: vi.fn(), listDingTalkGroupRoutes: vi.fn(), updateDingTalkGroupRoute: vi.fn(),
}));
vi.mock("@orvilo/core/api", () => ({ api: mocks }));
vi.mock("@orvilo/core/hooks", () => ({ useWorkspaceId: () => "workspace-1" }));
vi.mock("@orvilo/core/auth", () => ({
  useAuthStore: Object.assign(
    (select: (state: { user: { id: string } }) => unknown) => select({ user: { id: "user-1" } }),
    { getState: () => ({ user: { id: "user-1" } }) },
  ),
}));
vi.mock("@orvilo/core/workspace/hooks", () => ({
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
    installations: [{ id: "bot-1", agent_id: "agent-1", status: "installed", installed_at: "" }],
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

/**
 * `lobe: true` because `DingTalkTab` is Lobe now; every assertion after this
 * render must therefore start with an async query — until the bridge's module
 * resolves the tree is a `Suspense` fallback of `null`.
 */
function renderSettings() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  clients.push(client);
  renderWithI18n(
    <QueryClientProvider client={client}>
      <DingTalkTab />
    </QueryClientProvider>,
    { lobe: true },
  );
  return client;
}

/** The select's trigger, by the accessible name the row gives it. */
function agentSelect() {
  return screen.getByRole("combobox", { name: "Agent for Release team" });
}

/**
 * The popup is reached through `[role=listbox]`, which antd renders as a
 * clipped mirror of the options for assistive technology; the items a pointer
 * reaches are plain `div`s beside it that carry no role, so they are addressed
 * by their `title`. Same idiom as `preferences-tab.test.tsx`.
 */
async function openAgentMenu() {
  // `findBy`: this is the first query after a bridged render, and until the
  // bridge's module resolves the tree is a `Suspense` fallback of `null`.
  await userEvent.click(await screen.findByRole("combobox", { name: "Agent for Release team" }));
  const mirror = await screen.findByRole("listbox");
  return mirror.parentElement as HTMLElement;
}

async function pickAgent(optionTitle: string) {
  const menu = await openAgentMenu();
  await userEvent.click(within(menu).getByTitle(optionTitle));
}

/**
 * The selected label is asserted on the select's own root, not on the
 * combobox: that is a bare `<input>` whose text content is empty by
 * construction, and the label is a sibling node. It is scoped to `.ant-select`
 * because the popup's option nodes carry the same `title`, so a document-wide
 * query matches the trigger and the option both.
 */
function expectAssignedTo(agentName: string) {
  expect(agentSelect().closest(".ant-select")).toHaveTextContent(agentName);
}

describe("DingTalk group routing in Settings", () => {
  it("changes the selection only after the server accepts the reassignment", async () => {
    let accept!: (value: typeof originalRoute) => void;
    mocks.updateDingTalkGroupRoute.mockImplementationOnce(() => new Promise((resolve) => { accept = resolve; }));
    const client = renderSettings();
    await pickAgent("Release agent");
    expect(mocks.updateDingTalkGroupRoute).toHaveBeenCalledWith("workspace-1", "route-1", { agent_id: "agent-2" });
    expect(agentSelect()).toBeDisabled();
    expectAssignedTo("Default agent");
    currentRoute = { ...originalRoute, agent_id: "agent-2" };
    await act(async () => accept(currentRoute));
    await waitFor(() => expectAssignedTo("Release agent"));
    expect(client.getQueryData(dingtalkKeys.groupRoutes("workspace-1"))).toEqual({ routes: [currentRoute] });
    expect(toast.success).toHaveBeenCalled();
  });

  it("keeps the previous assignment after failure and lets the admin retry", async () => {
    mocks.updateDingTalkGroupRoute.mockRejectedValueOnce(new Error("temporarily unavailable"));
    renderSettings();
    await pickAgent("Release agent");
    await waitFor(() => expect(toast.error).toHaveBeenCalled());
    expectAssignedTo("Default agent");
    await pickAgent("Release agent");
    await waitFor(() => expectAssignedTo("Release agent"));
    expect(mocks.updateDingTalkGroupRoute).toHaveBeenCalledTimes(2);
  });

  it("does not announce success for a malformed update response", async () => {
    mocks.updateDingTalkGroupRoute.mockResolvedValueOnce({ id: "", agent_id: "" });
    renderSettings();
    await pickAgent("Release agent");
    await waitFor(() => expect(toast.error).toHaveBeenCalled());
    expect(toast.success).not.toHaveBeenCalled();
    expectAssignedTo("Default agent");
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
    // The section heading is the group's title now, so it is the group that
    // carries the name — `Form.Group` renders its label as a bare div with no
    // heading role, which is why `SettingsGroup` adds the wrapper.
    expect(await screen.findByRole("group", { name: "Group routing" })).toBeInTheDocument();
    expect(await screen.findByText("Release team")).toBeInTheDocument();
    expect(screen.queryByRole("combobox")).not.toBeInTheDocument();
    expect(mocks.updateDingTalkGroupRoute).not.toHaveBeenCalled();
  });

  it("excludes archived agents but keeps product-defined user agents eligible", async () => {
    renderSettings();
    const menu = await openAgentMenu();
    expect(within(menu).getByTitle("Release agent")).toBeInTheDocument();
    expect(within(menu).queryByTitle("Archived agent")).not.toBeInTheDocument();
    expect(within(menu).getByTitle("Patrick")).toBeInTheDocument();
  });

  it("does not query routes when the backend does not support them", async () => {
    mocks.listDingTalkInstallations.mockResolvedValue({
      configured: true, installations: [{ id: "bot-1", agent_id: "agent-1", status: "installed", installed_at: "" }],
    });
    renderSettings();
    await screen.findByText("Default agent");
    expect(screen.queryByRole("group", { name: "Group routing" })).not.toBeInTheDocument();
    expect(mocks.listDingTalkGroupRoutes).not.toHaveBeenCalled();
  });

  it("shows discovery instructions when a supported bot has no group routes", async () => {
    mocks.listDingTalkGroupRoutes.mockResolvedValue({ routes: [] });
    renderSettings();
    expect(await screen.findByText("No groups discovered yet")).toBeInTheDocument();
    expect(screen.queryByRole("combobox")).not.toBeInTheDocument();
  });

  it("only displays routes for the current workspace and visible installed installations", async () => {
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
