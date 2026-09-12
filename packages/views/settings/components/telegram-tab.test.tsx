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
const installationsRef = vi.hoisted(() => ({
  current: {
    installations: [] as unknown[],
    configured: true,
    install_supported: true,
  },
}));
const mockRegister = vi.hoisted(() => vi.fn());
const mockDeleteInstallation = vi.hoisted(() => vi.fn());
const mockOpenExternal = vi.hoisted(() => vi.fn());
const mockInvalidate = vi.hoisted(() => vi.fn());
const mockToastError = vi.hoisted(() => vi.fn());
const telegramQueryErrorRef = vi.hoisted(() => ({ current: false }));
const telegramQueryLoadingRef = vi.hoisted(() => ({ current: false }));

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
    if (key.includes("installations")) {
      return {
        data: telegramQueryLoadingRef.current ? undefined : installationsRef.current,
        isLoading: telegramQueryLoadingRef.current,
        isError: telegramQueryErrorRef.current,
      };
    }
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

vi.mock("@orvilo/core/telegram", () => ({
  telegramInstallationsOptions: () => ({
    queryKey: ["telegram", "installations"],
    queryFn: vi.fn(),
  }),
  telegramKeys: { installations: (wsId: string) => ["telegram", "installations", wsId] },
}));

vi.mock("@orvilo/core/api", () => ({
  api: {
    registerTelegramBot: mockRegister,
    deleteTelegramInstallation: mockDeleteInstallation,
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
import { TelegramAgentBindButton, TelegramTab } from "./telegram-tab";

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
 * it: the success path wants a generous *budget* (polling returns the instant
 * the node goes, so a slow close cannot fail it), and the failure path wants a
 * fixed *window* it spends in full, because no correct run ever closes and the
 * only question is whether a mutated build that closes would be caught.
 */
async function dialogLeavesWithin(timeout: number): Promise<boolean> {
  try {
    await waitFor(
      () => expect(screen.queryByText("Disconnect this Telegram bot?")).toBeNull(),
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

describe("Telegram deployment setup policy", () => {
  beforeEach(resetFixtures);

  it.each(["server_configured", "disabled"] as const)("keeps %s credential lifecycle read-only", async (mode) => {
    configStore.getState().setMessagingConfig({ mode, setupWritable: true, platforms: [] });
    // No bridge: on this path the component returns `MessagingSetupNotice`
    // (shadcn) or `null`, so no Lobe element renders and the negative assertion
    // below is decided by the component rather than by a pending module load.
    const entry = renderUI(<TelegramAgentBindButton agentId="agent-1" />, { lobe: false });
    expect(screen.queryByTestId("telegram-agent-connect")).toBeNull();
    expect(mockRegister).not.toHaveBeenCalled();
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
    renderUI(<><TelegramAgentBindButton agentId="agent-1" /><TelegramTab /></>);
    expect(await screen.findAllByRole("status", { name: "Connection status" })).toHaveLength(2);
    expect(screen.queryByRole("button", { name: /disconnect/i })).toBeNull();
    expect(mockDeleteInstallation).not.toHaveBeenCalled();
  });
});

function resetFixtures() {
  configStore.getState().setMessagingConfig({ mode: "managed", setupWritable: true, platforms: [] });
  vi.clearAllMocks();
  capturedConfirm.current = null;
  mockConfirmModal.mockImplementation((config: never) => {
    capturedConfirm.current = config as { onOk: () => Promise<unknown> };
    return actualConfirmModal.current?.(config);
  });
  membersRef.current = [{ user_id: "user-1", role: "owner" }];
  installationsRef.current = { installations: [], configured: true, install_supported: true };
  telegramQueryErrorRef.current = false;
  telegramQueryLoadingRef.current = false;
}

afterEach(cleanup);

describe("TelegramAgentBindButton", () => {
  beforeEach(resetFixtures);

  it("opens the connect dialog and submits the pasted bot token", async () => {
    mockRegister.mockResolvedValue({ id: "i1", agent_id: "agent-1", status: "installed" });
    renderUI(<TelegramAgentBindButton agentId="agent-1" agentName="Bot" />);
    await userEvent.click(await screen.findByTestId("telegram-agent-connect"));
    const tokenInput = await screen.findByTestId("telegram-bot-token");
    expect(tokenInput).toHaveAttribute("type", "password");
    await userEvent.type(tokenInput, "123456789:AAtesttoken");
    await userEvent.click(screen.getByTestId("telegram-connect-submit"));
    await waitFor(() =>
      expect(mockRegister).toHaveBeenCalledWith("workspace-1", "agent-1", {
        bot_token: "123456789:AAtesttoken",
      }),
    );
    expect(mockOpenExternal).not.toHaveBeenCalled();
  });

  it("opens the localized Telegram setup guide", async () => {
    renderUI(<TelegramAgentBindButton agentId="agent-1" agentName="Bot" />);
    await userEvent.click(await screen.findByTestId("telegram-agent-connect"));
    await userEvent.click(await screen.findByTestId("telegram-docs-link"));
    expect(mockOpenExternal).toHaveBeenCalledWith(
      "https://orvilo.aspectlylabs.com/docs/telegram-bot-integration",
    );
  });

  it("does not report success for a malformed install response", async () => {
    mockRegister.mockResolvedValue({});
    renderUI(<TelegramAgentBindButton agentId="agent-1" agentName="Bot" />);
    await userEvent.click(await screen.findByTestId("telegram-agent-connect"));
    await userEvent.type(await screen.findByTestId("telegram-bot-token"), "123456789:AAtesttoken");
    await userEvent.click(screen.getByTestId("telegram-connect-submit"));
    await waitFor(() => expect(mockToastError).toHaveBeenCalled());
    expect(mockInvalidate).not.toHaveBeenCalled();
  });

  it("shows the installation status (not the CTA) when the agent already has an installed bot", async () => {
    installationsRef.current = {
      installations: [
        { id: "i1", agent_id: "agent-1", status: "installed", bot_username: "my_bot" },
      ],
      configured: true,
      install_supported: true,
    };
    renderUI(<TelegramAgentBindButton agentId="agent-1" />);
    // The agent-detail half of this file keeps the shadcn primitives (its host
    // surface mounts no Lobe bridge), so the only thing awaiting the bridge here
    // is the render itself — this first query is what proves it landed.
    expect(await screen.findByTestId("telegram-agent-bot-installed")).toBeTruthy();
    expect(screen.getByTestId("telegram-agent-bot-disconnect")).toBeTruthy();
    expect(screen.queryByTestId("telegram-agent-connect")).toBeNull();
  });

  it("opens the installed bot in Telegram", async () => {
    installationsRef.current = {
      installations: [
        { id: "i1", agent_id: "agent-1", status: "installed", bot_username: "my_bot" },
      ],
      configured: true,
      install_supported: true,
    };
    renderUI(<TelegramAgentBindButton agentId="agent-1" />);

    await userEvent.click(await screen.findByRole("button", { name: /Open in Telegram/i }));

    expect(mockOpenExternal).toHaveBeenCalledWith("https://t.me/my_bot");
  });

  it("disconnects an agent bot only after confirmation", async () => {
    mockDeleteInstallation.mockResolvedValue(undefined);
    installationsRef.current = {
      installations: [
        { id: "i1", agent_id: "agent-1", status: "installed", bot_username: "my_bot" },
      ],
      configured: true,
      install_supported: true,
    };
    renderUI(<TelegramAgentBindButton agentId="agent-1" />);

    await userEvent.click(await screen.findByTestId("telegram-agent-bot-disconnect"));
    expect(mockDeleteInstallation).not.toHaveBeenCalled();
    const actions = await screen.findAllByRole("button", { name: /^Disconnect$/i });
    await userEvent.click(actions.at(-1)!);

    await waitFor(() =>
      expect(mockDeleteInstallation).toHaveBeenCalledWith("workspace-1", "i1"),
    );
    expect(mockInvalidate).toHaveBeenCalled();
  });

  it("keeps an agent bot connected when disconnect fails", async () => {
    mockDeleteInstallation.mockRejectedValue(new Error("network failed"));
    installationsRef.current = {
      installations: [
        { id: "i1", agent_id: "agent-1", status: "installed", bot_username: "my_bot" },
      ],
      configured: true,
      install_supported: true,
    };
    renderUI(<TelegramAgentBindButton agentId="agent-1" />);

    await userEvent.click(await screen.findByTestId("telegram-agent-bot-disconnect"));
    const actions = await screen.findAllByRole("button", { name: /^Disconnect$/i });
    await userEvent.click(actions.at(-1)!);

    await waitFor(() => expect(mockToastError).toHaveBeenCalledWith("network failed"));
    expect(screen.getByTestId("telegram-agent-bot-installed")).toBeTruthy();
    expect(mockInvalidate).not.toHaveBeenCalled();
  });

  it("renders nothing for a non-manager", () => {
    membersRef.current = [{ user_id: "user-1", role: "member" }];
    // `lobe: false` on purpose. The component returns `null` on this path, so
    // no Lobe element renders and no bridge is needed — and the emptiness
    // assertion stays meaningful, which it would not be under a bridge:
    // `ThemeProvider` renders a real `<div class="contents">`, so the container
    // would stop being empty for a reason that has nothing to do with the
    // component's behaviour.
    const { container } = renderUI(<TelegramAgentBindButton agentId="agent-1" />, { lobe: false });
    expect(container).toBeEmptyDOMElement();
  });

  it("renders nothing when install is unavailable and the agent is unbound", () => {
    installationsRef.current = { installations: [], configured: true, install_supported: false };
    // `lobe: false` for the same reason as the non-manager case above.
    const { container } = renderUI(<TelegramAgentBindButton agentId="agent-1" />, { lobe: false });
    expect(container).toBeEmptyDOMElement();
  });
});

describe("TelegramTab", () => {
  beforeEach(resetFixtures);

  it("shows a loading state without claiming Telegram is disabled", async () => {
    telegramQueryLoadingRef.current = true;
    renderUI(<TelegramTab />);
    expect(await screen.findByText("Loading…")).toBeTruthy();
    expect(screen.queryByText(/Telegram integration not enabled/i)).toBeNull();
  });

  it("surfaces the not-enabled notice when the deployment has no Telegram key", async () => {
    installationsRef.current = { installations: [], configured: false, install_supported: false };
    renderUI(<TelegramTab />);
    expect(await screen.findByText(/Telegram integration not enabled/i)).toBeTruthy();
  });

  it("shows the empty state when configured but nothing is connected", async () => {
    renderUI(<TelegramTab />);
    expect(await screen.findByText(/No bots installed yet/i)).toBeTruthy();
  });

  it("lists a connected installation with its agent name and a disconnect control", async () => {
    installationsRef.current = {
      installations: [
        { id: "i1", agent_id: "agent-7", status: "installed", bot_username: "my_bot" },
      ],
      configured: true,
      install_supported: true,
    };
    renderUI(<TelegramTab />);
    expect(await screen.findByText("Agent agent-7")).toBeTruthy();
    expect(screen.getByText("@my_bot")).toBeTruthy();
    expect(screen.getByText(/Disconnect/i)).toBeTruthy();
  });

  /**
   * The rethrow contract, asserted twice and at two layers, because the
   * DOM-level-only version of this test could not fail.
   *
   * `confirmModal` closes on the line after `onOk` unless `onOk` returns a
   * promise, and stays open when that promise rejects — so a disconnect that
   * fails must leave the confirmation up rather than dismiss as though the bot
   * were gone. The success test below is what gives the failure test's negative
   * assertion its meaning: it shows the dialog *does* leave this document.
   */
  it("disconnects only after the confirmation, and the dialog closes when it succeeds", async () => {
    mockDeleteInstallation.mockResolvedValue(undefined);
    installationsRef.current = {
      installations: [
        { id: "i1", agent_id: "agent-7", status: "installed", bot_username: "my_bot" },
      ],
      configured: true,
      install_supported: true,
    };
    renderUI(<TelegramTab />);
    await userEvent.click(await screen.findByRole("button", { name: /Disconnect/i }));
    expect(mockDeleteInstallation).not.toHaveBeenCalled();

    await screen.findByText("Disconnect this Telegram bot?");
    // Two buttons now carry this name — the row's and the dialog's — and the
    // dialog's is the last in document order (it is portalled to the end of
    // `<body>`), so `.at(-1)` is the one that runs the request.
    const confirmButtons = await screen.findAllByRole("button", { name: /^Disconnect$/ });
    await userEvent.click(confirmButtons.at(-1)!);
    await waitFor(() =>
      expect(mockDeleteInstallation).toHaveBeenCalledWith("workspace-1", "i1"),
    );
    expect(mockInvalidate).toHaveBeenCalled();
    expect(await dialogLeavesWithin(CLOSE_BUDGET_MS)).toBe(true);
  });

  it("keeps the confirmation up when the disconnect fails", async () => {
    mockDeleteInstallation.mockRejectedValue(new Error("network failed"));
    installationsRef.current = {
      installations: [
        { id: "i1", agent_id: "agent-7", status: "installed", bot_username: "my_bot" },
      ],
      configured: true,
      install_supported: true,
    };
    renderUI(<TelegramTab />);
    await userEvent.click(await screen.findByRole("button", { name: /Disconnect/i }));
    await screen.findByText("Disconnect this Telegram bot?");
    const confirmButtons = await screen.findAllByRole("button", { name: /^Disconnect$/ });
    await userEvent.click(confirmButtons.at(-1)!);

    await waitFor(() => expect(mockToastError).toHaveBeenCalledWith("network failed"));
    expect(screen.getByText("@my_bot")).toBeTruthy();
    expect(mockInvalidate).not.toHaveBeenCalled();
    expect(await dialogLeavesWithin(CLOSE_WINDOW_MS)).toBe(false);
  });

  it("hands confirmModal an onOk that rejects when the disconnect fails", async () => {
    mockDeleteInstallation.mockRejectedValue(new Error("network failed"));
    installationsRef.current = {
      installations: [
        { id: "i1", agent_id: "agent-7", status: "installed", bot_username: "my_bot" },
      ],
      configured: true,
      install_supported: true,
    };
    renderUI(<TelegramTab />);
    await userEvent.click(await screen.findByRole("button", { name: /Disconnect/i }));
    expect(capturedConfirm.current).not.toBeNull();

    await expect(capturedConfirm.current?.onOk()).rejects.toThrow("network failed");
    expect(mockToastError).toHaveBeenCalledWith("network failed");
  });

  // Malformed-response defense (CLAUDE.md → API Compatibility): a response
  // missing `installations` must not crash the panel.
  it("tolerates a malformed installations response", async () => {
    installationsRef.current = { configured: true } as never;
    renderUI(<TelegramTab />);
    expect(await screen.findByText(/No bots installed yet/i)).toBeTruthy();
  });

  it("shows a load error instead of pretending Telegram is disabled", async () => {
    telegramQueryErrorRef.current = true;
    renderUI(<TelegramTab />);
    expect(await screen.findByText(/Failed to load Telegram installations/i)).toBeTruthy();
    expect(screen.queryByText(/Telegram integration not enabled/i)).toBeNull();
  });
});
