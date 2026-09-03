// @vitest-environment jsdom

import { type ReactNode } from "react";
import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nProvider } from "@patchbay/core/i18n/react";
import enCommon from "../../locales/en/common.json";
import enSettings from "../../locales/en/settings.json";

type MemberRole = "owner" | "admin" | "member" | "guest";

const membersRef = vi.hoisted(() => ({
  current: [{ user_id: "user-1", role: "owner" as MemberRole }],
}));
const installationsRef = vi.hoisted((): { current: {
  installations: unknown[];
  configured: boolean;
  install_supported: boolean;
  managed_supported?: boolean;
} } => ({
  current: {
    installations: [] as unknown[],
    configured: true,
    install_supported: true,
    managed_supported: false,
  },
}));
const mockRegisterBYO = vi.hoisted(() => vi.fn());
const mockBeginManaged = vi.hoisted(() => vi.fn());
const mockDeleteInstallation = vi.hoisted(() => vi.fn());
const mockOpenExternal = vi.hoisted(() => vi.fn());
const mockInvalidate = vi.hoisted(() => vi.fn());
const queryErrorRef = vi.hoisted(() => ({ current: false }));

vi.mock("@tanstack/react-query", () => ({
  useQuery: (opts: { queryKey: unknown[]; enabled?: boolean }) => {
    if (opts.enabled === false) return { data: undefined, isLoading: false };
    const key = JSON.stringify(opts.queryKey);
    if (key.includes("members")) return { data: membersRef.current, isLoading: false };
    if (key.includes("installations")) return { data: installationsRef.current, isLoading: false, isError: queryErrorRef.current };
    return { data: undefined, isLoading: false };
  },
  useQueryClient: () => ({ invalidateQueries: mockInvalidate }),
  queryOptions: <T,>(opts: T) => opts,
}));

vi.mock("@patchbay/core/hooks", () => ({ useWorkspaceId: () => "workspace-1" }));

vi.mock("@patchbay/core/workspace/queries", () => ({
  memberListOptions: () => ({ queryKey: ["members"], queryFn: vi.fn() }),
}));

vi.mock("@patchbay/core/workspace/hooks", () => ({
  useActorName: () => ({
    getAgentName: (agentId: string) => `Agent ${agentId}`,
    getMemberName: () => "Unknown",
    getTeamName: () => "Unknown Team",
    getActorName: () => "Unknown",
    getActorInitials: () => "??",
    getActorAvatarUrl: () => null,
  }),
}));

vi.mock("../../common/actor-avatar", () => ({
  ActorAvatar: ({ actorId }: { actorId: string }) => (
    <span data-testid="actor-avatar" data-actor-id={actorId} />
  ),
}));

vi.mock("@patchbay/core/slack", () => ({
  slackInstallationsOptions: () => ({
    queryKey: ["slack", "installations"],
    queryFn: vi.fn(),
  }),
  slackKeys: { installations: (wsId: string) => ["slack", "installations", wsId] },
}));

vi.mock("@patchbay/core/api", () => ({
  api: {
    registerSlackBYO: mockRegisterBYO,
    beginManagedSlackInstall: mockBeginManaged,
    deleteSlackInstallation: mockDeleteInstallation,
  },
}));

vi.mock("@patchbay/core/auth", () => {
  const useAuthStore = Object.assign(
    (sel?: (s: { user: { id: string } }) => unknown) =>
      sel ? sel({ user: { id: "user-1" } }) : { user: { id: "user-1" } },
    { getState: () => ({ user: { id: "user-1" } }) },
  );
  return { useAuthStore };
});

vi.mock("sonner", () => ({
  toast: { success: vi.fn(), error: vi.fn(), message: vi.fn() },
}));

vi.mock("../../platform", () => ({ openExternal: mockOpenExternal }));

import { SlackAgentBindButton, SlackTab } from "./slack-tab";

const TEST_RESOURCES = { en: { common: enCommon, settings: enSettings } };

function renderUI(children: ReactNode) {
  return render(
    <I18nProvider locale="en" resources={TEST_RESOURCES}>
      {children}
    </I18nProvider>,
  );
}

function resetFixtures() {
  vi.clearAllMocks();
  queryErrorRef.current = false;
  membersRef.current = [{ user_id: "user-1", role: "owner" }];
  installationsRef.current = { installations: [], configured: true, install_supported: true };
}

describe("SlackAgentBindButton", () => {
  beforeEach(resetFixtures);

  it("opens the BYO dialog and submits the pasted bot + app tokens", async () => {
    mockRegisterBYO.mockResolvedValue({ id: "i1", agent_id: "agent-1", status: "installed" });
    renderUI(<SlackAgentBindButton agentId="agent-1" agentName="Bot" />);
    await userEvent.click(screen.getByTestId("slack-agent-connect"));
    const botInput = await screen.findByTestId("slack-byo-bot-token");
    await userEvent.type(botInput, "xoxb-bot");
    await userEvent.type(screen.getByTestId("slack-byo-app-token"), "xapp-1-A0X-1-secret");
    await userEvent.click(screen.getByTestId("slack-byo-submit"));
    await waitFor(() =>
      expect(mockRegisterBYO).toHaveBeenCalledWith("workspace-1", "agent-1", {
        bot_token: "xoxb-bot",
        app_token: "xapp-1-A0X-1-secret",
      }),
    );
    // No OAuth redirect anymore — install is a direct API call.
    expect(mockOpenExternal).not.toHaveBeenCalled();
  });

  it("keeps an unobserved installation manageable without claiming it is connected", () => {
    installationsRef.current = {
      installations: [{ id: "i1", agent_id: "agent-1", status: "installed", team_id: "T1" }],
      configured: true,
      install_supported: true,
    };
    renderUI(<SlackAgentBindButton agentId="agent-1" />);
    expect(screen.getByTestId("slack-agent-bot-connected")).toBeTruthy();
    expect(screen.getByTestId("slack-agent-bot-disconnect")).toBeTruthy();
    expect(screen.getByRole("status", { name: "Connection status" }).textContent).toBe("Status unavailable");
    expect(screen.queryByTestId("slack-agent-connect")).toBeNull();
  });

  it("renders nothing for a non-manager", () => {
    membersRef.current = [{ user_id: "user-1", role: "member" }];
    const { container } = renderUI(<SlackAgentBindButton agentId="agent-1" />);
    expect(container).toBeEmptyDOMElement();
  });

  it.each([
    ["healthy", "Connected"],
    ["offline", "Disconnected"],
    ["starting", "Connecting"],
    ["future_state", "Status unavailable"],
  ])("renders %s from the server while preserving the management action", (state, label) => {
    installationsRef.current = {
      installations: [{ id: "i1", agent_id: "agent-1", status: "installed", team_id: "T1",
        runtime: { state, observedAt: "2026-09-03T12:00:00Z", errorCode: null } }],
      configured: true, install_supported: true,
    };
    renderUI(<SlackAgentBindButton agentId="agent-1" />);
    expect(screen.getByRole("status", { name: "Connection status" }).textContent).toBe(label);
    expect(screen.getByTestId("slack-agent-bot-disconnect")).toBeTruthy();
    expect(screen.queryByTestId("slack-agent-connect")).toBeNull();
  });

  it("renders nothing when install is unavailable and the agent is unbound", () => {
    installationsRef.current = { installations: [], configured: true, install_supported: false };
    const { container } = renderUI(<SlackAgentBindButton agentId="agent-1" />);
    expect(container).toBeEmptyDOMElement();
  });
});

describe("SlackTab", () => {
  beforeEach(resetFixtures);

  it("does not reuse a cached connection confirmation after the status query fails", () => {
    installationsRef.current = {
      installations: [{ id: "i1", agent_id: "agent-1", status: "installed", team_id: "T1",
        installed_at: "2026-09-03T12:00:00Z",
        runtime: { state: "healthy", observedAt: "2026-09-03T12:00:00Z", errorCode: null } }],
      configured: true, install_supported: true,
    };
    queryErrorRef.current = true;
    renderUI(<SlackTab />);
    expect(screen.getByRole("status", { name: "Connection status" }).textContent).toContain("Status unavailable");
    expect(screen.getByRole("button", { name: "Disconnect" })).toBeTruthy();
  });

  it("surfaces the not-enabled notice when the deployment has no Slack key", () => {
    installationsRef.current = { installations: [], configured: false, install_supported: false };
    renderUI(<SlackTab />);
    expect(screen.getByText(/Slack integration not enabled/i)).toBeTruthy();
  });

  it("shows the empty state when configured but nothing is connected", () => {
    renderUI(<SlackTab />);
    expect(screen.getByText(/No bots installed yet/i)).toBeTruthy();
  });

  it("lists a connected installation with its agent name and a disconnect control", () => {
    installationsRef.current = {
      installations: [{ id: "i1", agent_id: "agent-7", status: "installed", team_id: "T1" }],
      configured: true,
      install_supported: true,
    };
    renderUI(<SlackTab />);
    expect(screen.getByText("Agent agent-7")).toBeTruthy();
    expect(screen.getByText(/Disconnect/i)).toBeTruthy();
  });

  it("shows the managed connect button only when the hosted path is supported", () => {
    installationsRef.current = {
      installations: [],
      configured: true,
      install_supported: true,
      managed_supported: true,
    };
    renderUI(<SlackTab />);
    expect(screen.getByTestId("slack-managed-connect")).toBeTruthy();
  });

  it("hides the managed connect button without hosted credentials", () => {
    installationsRef.current = {
      installations: [],
      configured: true,
      install_supported: true,
      managed_supported: false,
    };
    renderUI(<SlackTab />);
    expect(screen.queryByTestId("slack-managed-connect")).toBeNull();
  });

  it("starts a managed install against the workspace and follows the authorize URL", async () => {
    mockBeginManaged.mockResolvedValue({
      authorize_url: "https://slack.com/oauth/v2/authorize?state=abc",
      state: "abc",
      expires_at: "2026-09-02T00:10:00Z",
    });
    installationsRef.current = {
      installations: [],
      configured: true,
      install_supported: true,
      managed_supported: true,
    };
    renderUI(<SlackTab />);
    await userEvent.click(screen.getByTestId("slack-managed-connect"));
    await waitFor(() => {
      expect(mockBeginManaged).toHaveBeenCalledWith("workspace-1", expect.any(String));
    });
  });

  it("toasts when the managed begin fails", async () => {
    const { toast } = await import("sonner");
    mockBeginManaged.mockRejectedValue(new Error("nope"));
    installationsRef.current = {
      installations: [],
      configured: true,
      install_supported: true,
      managed_supported: true,
    };
    renderUI(<SlackTab />);
    await userEvent.click(screen.getByTestId("slack-managed-connect"));
    await waitFor(() => {
      expect(toast.error).toHaveBeenCalled();
    });
  });

  it("renders a workspace-level install under its Slack team, not an agent", () => {
    installationsRef.current = {
      installations: [
        {
          id: "i9",
          agent_id: "00000000-0000-0000-0000-000000000000",
          status: "installed",
          team_id: "T1",
          bot_user_id: "UBOT",
        },
      ],
      configured: true,
      install_supported: true,
      managed_supported: true,
    };
    renderUI(<SlackTab />);
    expect(screen.getByText("Slack workspace T1")).toBeTruthy();
    expect(screen.queryByTestId("actor-avatar")).toBeNull();
    // An active managed install replaces the connect button.
    expect(screen.queryByTestId("slack-managed-connect")).toBeNull();
  });
});
