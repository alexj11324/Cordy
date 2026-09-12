// @vitest-environment jsdom

import { type ReactNode } from "react";
import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { cleanup, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { configStore } from "@orvilo/core/config";

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
const mockToastError = vi.hoisted(() => vi.fn());
const queryErrorRef = vi.hoisted(() => ({ current: false }));

/**
 * `confirmModal` is imperative and module-level, so the config the wrapper
 * hands it — and therefore the promise `onOk` returns — is the only place the
 * rethrow contract is observable. Recorded here and delegated to the real one,
 * because the tests below also drive the dialog through the UI.
 */
const capturedConfirm = vi.hoisted(() => ({
  current: null as { onOk: () => Promise<unknown> } | null,
}));
const mockConfirmModal = vi.hoisted(() => vi.fn());
const actualConfirmModal = vi.hoisted(() => ({
  current: null as null | ((config: never) => unknown),
}));

vi.mock("@lobehub/ui/base-ui", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@lobehub/ui/base-ui")>();
  actualConfirmModal.current = actual.confirmModal as never;
  return { ...actual, confirmModal: mockConfirmModal };
});

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

vi.mock("@orvilo/core/hooks", () => ({ useWorkspaceId: () => "workspace-1" }));

vi.mock("@orvilo/core/workspace/queries", () => ({
  memberListOptions: () => ({ queryKey: ["members"], queryFn: vi.fn() }),
}));

vi.mock("@orvilo/core/workspace/hooks", () => ({
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

vi.mock("@orvilo/core/slack", () => ({
  slackInstallationsOptions: () => ({
    queryKey: ["slack", "installations"],
    queryFn: vi.fn(),
  }),
  slackKeys: { installations: (wsId: string) => ["slack", "installations", wsId] },
}));

vi.mock("@orvilo/core/api", () => ({
  api: {
    registerSlackBYO: mockRegisterBYO,
    beginManagedSlackInstall: mockBeginManaged,
    deleteSlackInstallation: mockDeleteInstallation,
  },
}));

vi.mock("@orvilo/core/auth", () => {
  const useAuthStore = Object.assign(
    (sel?: (s: { user: { id: string } }) => unknown) =>
      sel ? sel({ user: { id: "user-1" } }) : { user: { id: "user-1" } },
    { getState: () => ({ user: { id: "user-1" } }) },
  );
  return { useAuthStore };
});

vi.mock("sonner", () => ({
  toast: { success: vi.fn(), error: mockToastError, message: vi.fn() },
}));

vi.mock("../../platform", () => ({ openExternal: mockOpenExternal }));

import { renderWithI18n } from "../../test/i18n";
import { SlackAgentBindButton, SlackTab } from "./slack-tab";

/**
 * `lobe: true` mounts the theme bridge on demand, so everything after a
 * bridged render must start with an async query — until the bridge's module
 * resolves, the tree is a `Suspense` fallback of `null` and a synchronous
 * `getBy*` would fail. The one case that passes `lobe: false` is a render that
 * produces no Lobe element at all; see the note on those tests.
 */
function renderUI(children: ReactNode, { lobe = true }: { lobe?: boolean } = {}) {
  return renderWithI18n(<>{children}</>, { lobe });
}

/**
 * Whether the confirm dialog leaves the document within `timeout`. Polls —
 * `waitFor` re-runs the callback until it stops throwing — because a check that
 * samples at a fixed moment cannot tell an open dialog from a closing one.
 *
 * The timeout is a parameter because the two callers want opposite things from
 * it, and the failure directions are not symmetric:
 *
 *   - the **success** path passes a generous budget. Polling stops the moment
 *     the node goes, so a fast close costs nothing, and a slow one cannot fail
 *     it — the expensive direction is a correct build failing under load.
 *   - the **failure** path passes a fixed window. It spends that whole window
 *     waiting, and a longer one is not better: no correct run ever closes, so
 *     the only question is whether a mutated build that closes would be caught.
 */
async function dialogLeavesWithin(timeout: number): Promise<boolean> {
  try {
    await waitFor(
      () => expect(screen.queryByText("Disconnect this Slack bot?")).toBeNull(),
      { timeout },
    );
    return true;
  } catch {
    return false;
  }
}

/** A dismissal window: long enough for a close, short enough to be an assertion. */
const CLOSE_WINDOW_MS = 1_200;
/** A budget, not a window: the success path must not fail a loaded runner. */
const CLOSE_BUDGET_MS = 8_000;

describe("Slack deployment setup policy", () => {
  beforeEach(resetFixtures);

  it.each(["server_configured", "disabled"] as const)("keeps %s credential lifecycle read-only", async (mode) => {
    configStore.getState().setMessagingConfig({ mode, setupWritable: true, platforms: [] });
    // No bridge: on this path the component returns `MessagingSetupNotice`
    // (shadcn) or `null`, so no Lobe element renders and the negative assertion
    // below is decided by the component rather than by a pending module load.
    const entry = renderUI(<SlackAgentBindButton agentId="agent-1" />, { lobe: false });
    expect(screen.queryByTestId("slack-agent-connect")).toBeNull();
    expect(mockRegisterBYO).not.toHaveBeenCalled();
    entry.unmount();

    installationsRef.current = {
      configured: false,
      install_supported: true,
      installations: [{
        id: "existing-installation", agent_id: "agent-1", status: "installed",
        installed_at: "2026-09-10T00:00:00Z", region: "feishu",
        app_id: "app-1", team_id: "team-1", bot_id: "bot-1",
      }],
    };
    renderUI(<><SlackAgentBindButton agentId="agent-1" /><SlackTab /></>);
    expect(await screen.findAllByRole("status", { name: "Connection status" })).toHaveLength(2);
    expect(screen.queryByRole("button", { name: /disconnect/i })).toBeNull();
    expect(mockDeleteInstallation).not.toHaveBeenCalled();
  });
});

function resetFixtures() {
  configStore.getState().setMessagingConfig({ mode: "managed", setupWritable: true, platforms: [] });
  vi.clearAllMocks();
  queryErrorRef.current = false;
  capturedConfirm.current = null;
  mockConfirmModal.mockImplementation((config: never) => {
    capturedConfirm.current = config as { onOk: () => Promise<unknown> };
    return actualConfirmModal.current?.(config);
  });
  membersRef.current = [{ user_id: "user-1", role: "owner" }];
  installationsRef.current = { installations: [], configured: true, install_supported: true };
}

afterEach(cleanup);

describe("SlackAgentBindButton", () => {
  beforeEach(resetFixtures);

  it("opens the BYO dialog and submits the pasted bot + app tokens", async () => {
    mockRegisterBYO.mockResolvedValue({ id: "i1", agent_id: "agent-1", status: "installed" });
    renderUI(<SlackAgentBindButton agentId="agent-1" agentName="Bot" />);
    await userEvent.click(await screen.findByTestId("slack-agent-connect"));
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

  it("keeps an unobserved installation manageable without claiming it is connected", async () => {
    installationsRef.current = {
      installations: [{ id: "i1", agent_id: "agent-1", status: "installed", team_id: "T1" }],
      configured: true,
      install_supported: true,
    };
    renderUI(<SlackAgentBindButton agentId="agent-1" />);
    // The agent-detail half of this file keeps the shadcn primitives (its host
    // surface mounts no Lobe bridge), so the only thing awaiting the bridge here
    // is the render itself — this first query is what proves it landed.
    expect(await screen.findByTestId("slack-agent-bot-installed")).toBeTruthy();
    expect(screen.getByTestId("slack-agent-bot-disconnect")).toBeTruthy();
    expect(screen.getByRole("status", { name: "Connection status" }).textContent).toBe("Status unavailable");
    expect(screen.queryByTestId("slack-agent-connect")).toBeNull();
  });

  it("renders nothing for a non-manager", () => {
    membersRef.current = [{ user_id: "user-1", role: "member" }];
    // `lobe: false` on purpose. The component returns `null` on this path, so
    // no Lobe element renders and no bridge is needed — and the emptiness
    // assertion stays meaningful, which it would not be under a bridge:
    // `ThemeProvider` renders a real `<div class="contents">`, so the container
    // would stop being empty for a reason that has nothing to do with the
    // component's behaviour.
    const { container } = renderUI(<SlackAgentBindButton agentId="agent-1" />, { lobe: false });
    expect(container).toBeEmptyDOMElement();
  });

  it.each([
    ["healthy", "Connected"],
    ["offline", "Disconnected"],
    ["starting", "Connecting"],
    ["future_state", "Status unavailable"],
  ])("renders %s from the server while preserving the management action", async (state, label) => {
    installationsRef.current = {
      installations: [{ id: "i1", agent_id: "agent-1", status: "installed", team_id: "T1",
        runtime: { state, observedAt: "2026-09-03T12:00:00Z", errorCode: null } }],
      configured: true, install_supported: true,
    };
    renderUI(<SlackAgentBindButton agentId="agent-1" />);
    expect((await screen.findByRole("status", { name: "Connection status" })).textContent).toBe(label);
    expect(screen.getByTestId("slack-agent-bot-disconnect")).toBeTruthy();
    expect(screen.queryByTestId("slack-agent-connect")).toBeNull();
  });

  it("renders nothing when install is unavailable and the agent is unbound", () => {
    installationsRef.current = { installations: [], configured: true, install_supported: false };
    // `lobe: false` for the same reason as the non-manager case above.
    const { container } = renderUI(<SlackAgentBindButton agentId="agent-1" />, { lobe: false });
    expect(container).toBeEmptyDOMElement();
  });
});

describe("SlackTab", () => {
  beforeEach(resetFixtures);

  it("does not reuse a cached connection confirmation after the status query fails", async () => {
    installationsRef.current = {
      installations: [{ id: "i1", agent_id: "agent-1", status: "installed", team_id: "T1",
        installed_at: "2026-09-03T12:00:00Z",
        runtime: { state: "healthy", observedAt: "2026-09-03T12:00:00Z", errorCode: null } }],
      configured: true, install_supported: true,
    };
    queryErrorRef.current = true;
    renderUI(<SlackTab />);
    expect((await screen.findByRole("status", { name: "Connection status" })).textContent).toContain("Status unavailable");
    expect(screen.getByRole("button", { name: "Disconnect" })).toBeTruthy();
  });

  it("surfaces the not-enabled notice when the deployment has no Slack key", async () => {
    installationsRef.current = { installations: [], configured: false, install_supported: false };
    renderUI(<SlackTab />);
    expect(await screen.findByText(/Slack integration not enabled/i)).toBeTruthy();
  });

  it("shows the empty state when configured but nothing is connected", async () => {
    renderUI(<SlackTab />);
    expect(await screen.findByText(/No bots installed yet/i)).toBeTruthy();
  });

  it("lists a connected installation with its agent name and a disconnect control", async () => {
    installationsRef.current = {
      installations: [{ id: "i1", agent_id: "agent-7", status: "installed", team_id: "T1" }],
      configured: true,
      install_supported: true,
    };
    renderUI(<SlackTab />);
    expect(await screen.findByText("Agent agent-7")).toBeTruthy();
    expect(screen.getByText(/Disconnect/i)).toBeTruthy();
  });

  it("shows the managed connect button only when the hosted path is supported", async () => {
    installationsRef.current = {
      installations: [],
      configured: true,
      install_supported: true,
      managed_supported: true,
    };
    renderUI(<SlackTab />);
    expect(await screen.findByTestId("slack-managed-connect")).toBeTruthy();
  });

  it("hides the managed connect button without hosted credentials", async () => {
    installationsRef.current = {
      installations: [],
      configured: true,
      install_supported: true,
      managed_supported: false,
    };
    renderUI(<SlackTab />);
    await screen.findByText(/No bots installed yet/i);
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
    await userEvent.click(await screen.findByTestId("slack-managed-connect"));
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
    await userEvent.click(await screen.findByTestId("slack-managed-connect"));
    await waitFor(() => {
      expect(toast.error).toHaveBeenCalled();
    });
  });

  it("renders a workspace-level install under its Slack team, not an agent", async () => {
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
    expect(await screen.findByText("Slack workspace T1")).toBeTruthy();
    expect(screen.queryByTestId("actor-avatar")).toBeNull();
    // An installed managed bot replaces the connect button.
    expect(screen.queryByTestId("slack-managed-connect")).toBeNull();
  });

  /**
   * The rethrow contract, asserted twice and at two layers, because the
   * DOM-level-only version of this test could not fail.
   *
   * `confirmModal` closes on the line after `onOk` unless `onOk` returns a
   * promise, and stays open when that promise rejects — so a disconnect that
   * fails must leave the confirmation up rather than dismiss as though the bot
   * were gone.
   *
   * The first test below keeps the DOM spelling but waits a window in which the
   * success path is shown to close; without that demonstration "the title is
   * still in the document" could be true because nothing ever leaves it. The
   * second drops the DOM entirely: the contract lives in the promise `onOk`
   * returns, and that is directly readable off the config the wrapper handed
   * the library.
   */
  it("disconnects only after the confirmation, and the dialog closes when it succeeds", async () => {
    mockDeleteInstallation.mockResolvedValue(undefined);
    installationsRef.current = {
      installations: [{ id: "i1", agent_id: "agent-7", status: "installed", team_id: "T1" }],
      configured: true,
      install_supported: true,
    };
    renderUI(<SlackTab />);
    await userEvent.click(await screen.findByRole("button", { name: "Disconnect" }));
    expect(mockDeleteInstallation).not.toHaveBeenCalled();

    await screen.findByText("Disconnect this Slack bot?");
    // Two buttons now carry this name — the row's and the dialog's — and the
    // dialog's is the last in document order (it is portalled to the end of
    // `<body>`), so `.at(-1)` is the one that runs the request.
    const confirmButtons = await screen.findAllByRole("button", { name: /^Disconnect$/ });
    await userEvent.click(confirmButtons.at(-1)!);
    await waitFor(() => {
      expect(mockDeleteInstallation).toHaveBeenCalledWith("workspace-1", "i1");
    });
    expect(mockInvalidate).toHaveBeenCalled();
    expect(await dialogLeavesWithin(CLOSE_BUDGET_MS)).toBe(true);
  });

  it("keeps the confirmation up when the disconnect request fails", async () => {
    mockDeleteInstallation.mockRejectedValue(new Error("network failed"));
    installationsRef.current = {
      installations: [{ id: "i1", agent_id: "agent-7", status: "installed", team_id: "T1" }],
      configured: true,
      install_supported: true,
    };
    renderUI(<SlackTab />);
    await userEvent.click(await screen.findByRole("button", { name: "Disconnect" }));
    await screen.findByText("Disconnect this Slack bot?");
    const confirmButtons = await screen.findAllByRole("button", { name: /^Disconnect$/ });
    await userEvent.click(confirmButtons.at(-1)!);

    await waitFor(() => expect(mockToastError).toHaveBeenCalledWith("network failed"));
    expect(await dialogLeavesWithin(CLOSE_WINDOW_MS)).toBe(false);
  });

  it("hands confirmModal an onOk that rejects when the disconnect fails", async () => {
    mockDeleteInstallation.mockRejectedValue(new Error("network failed"));
    installationsRef.current = {
      installations: [{ id: "i1", agent_id: "agent-7", status: "installed", team_id: "T1" }],
      configured: true,
      install_supported: true,
    };
    renderUI(<SlackTab />);
    await userEvent.click(await screen.findByRole("button", { name: "Disconnect" }));
    expect(capturedConfirm.current).not.toBeNull();

    await expect(capturedConfirm.current?.onOk()).rejects.toThrow("network failed");
    expect(mockToastError).toHaveBeenCalledWith("network failed");
  });
});
