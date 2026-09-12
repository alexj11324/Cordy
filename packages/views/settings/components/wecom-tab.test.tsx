// @vitest-environment jsdom

import { type ReactNode } from "react";
import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { cleanup, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { configStore } from "@orvilo/core/config";
import enSettings from "../../locales/en/settings.json";

// wecom-tab.test.tsx — the Settings → Integrations WeCom panel and the per-agent
// bind button. Mirrors dingtalk-tab.test.tsx. Covers the admin gate, the BYO
// install dialog + credential trimming, the connected badge, the not-enabled /
// empty states, and confirm-before-disconnect.
//
// Co-authored analysis: seacen (PR #5833 review).

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
const mockRegisterBYO = vi.hoisted(() => vi.fn());
const mockDeleteInstallation = vi.hoisted(() => vi.fn());
const mockInvalidate = vi.hoisted(() => vi.fn());

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
    if (key.includes("installations")) return { data: installationsRef.current, isLoading: false };
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

vi.mock("@orvilo/core/wecom", () => ({
  wecomInstallationsOptions: () => ({
    queryKey: ["wecom", "installations"],
    queryFn: vi.fn(),
  }),
  wecomKeys: { installations: (wsId: string) => ["wecom", "installations", wsId] },
}));

vi.mock("@orvilo/core/api", () => ({
  api: {
    registerWecomBYO: mockRegisterBYO,
    deleteWecomInstallation: mockDeleteInstallation,
  },
  // The real one digs the code out of an ApiError body; the fake reads it off
  // whatever the test threw, so a test can drive one branch of the switch.
  errorCode: (e: unknown) =>
    e && typeof e === "object" ? (e as { code?: string }).code : undefined,
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
  toast: { success: vi.fn(), error: vi.fn(), message: vi.fn() },
}));

import { toast } from "sonner";
import { renderWithI18n } from "../../test/i18n";
import { WecomAgentBindButton, WecomTab } from "./wecom-tab";

afterEach(cleanup);

/**
 * `lobe: true` mounts the theme bridge on demand, so everything after a
 * bridged render must start with an async query — until the bridge's module
 * resolves the tree is a `Suspense` fallback of `null` and a synchronous
 * `getBy*` would fail.
 *
 * The flag defaults to **off** here rather than on, which is the opposite of
 * the Slack/Telegram suite's helper, and deliberately so: most of this file
 * renders `WecomAgentBindButton`, whose host surface (the agent detail page)
 * mounts no bridge and which therefore renders no Lobe element at all. A
 * bridged render would make the two `toBeEmptyDOMElement()` assertions below
 * pass *vacuously* — the container is empty while the bridge module resolves —
 * which is the "guard that cannot fail" shape. Renders that can reach a
 * migrated component pass `lobe: true` explicitly, and a render that reaches
 * one without the bridge throws loudly rather than failing silently.
 */
function renderUI(children: ReactNode, { lobe = false }: { lobe?: boolean } = {}) {
  return renderWithI18n(<>{children}</>, { lobe });
}

/**
 * Whether the confirm dialog leaves the document within `timeout`. Polls —
 * `waitFor` re-runs the callback until it stops throwing — because a check that
 * samples at a fixed moment cannot tell an open dialog from a closing one.
 *
 * The timeout is a parameter because the two callers want opposite things from
 * it: the **success** path passes a generous budget (polling stops the moment
 * the node goes, so a slow close cannot fail it), and the **failure** path
 * passes a fixed window it spends in full (no correct run ever closes, so the
 * only question is whether a mutated build that closes would be caught).
 */
async function dialogLeavesWithin(timeout: number): Promise<boolean> {
  try {
    await waitFor(
      () => expect(screen.queryByText("Disconnect this WeCom bot?")).toBeNull(),
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

describe("Wecom deployment setup policy", () => {
  beforeEach(resetFixtures);

  it.each(["server_configured", "disabled"] as const)("keeps %s credential lifecycle read-only", async (mode) => {
    configStore.getState().setMessagingConfig({ mode, setupWritable: true, platforms: [] });
    // No bridge: this render produces no Lobe element, so the negative
    // assertion is decided by the component rather than by a bridge module that
    // has not resolved yet.
    const entry = renderUI(<WecomAgentBindButton agentId="agent-1" />, { lobe: false });
    expect(screen.queryByTestId("wecom-agent-connect")).toBeNull();
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
    renderUI(<><WecomAgentBindButton agentId="agent-1" /><WecomTab /></>, { lobe: true });
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
}

describe("WecomAgentBindButton", () => {
  beforeEach(resetFixtures);

  it("keeps the form open when a malformed response cannot confirm installation", async () => {
    mockRegisterBYO.mockResolvedValue({ id: "", status: "revoked" });
    renderUI(<WecomAgentBindButton />);
    await userEvent.click(screen.getByTestId("wecom-agent-connect"));
    await userEvent.type(await screen.findByTestId("wecom-byo-bot-id"), "aibot-test");
    await userEvent.type(screen.getByTestId("wecom-byo-secret"), "test-secret");
    await userEvent.click(screen.getByTestId("wecom-byo-submit"));
    await waitFor(() => expect(toast.error).toHaveBeenCalledWith(enSettings.wecom.byo_failed_toast));
    expect(mockRegisterBYO).toHaveBeenCalledTimes(1);
    expect(toast.success).not.toHaveBeenCalled();
    expect(mockInvalidate).not.toHaveBeenCalled();
    expect(screen.getByTestId("wecom-byo-dialog")).toBeInTheDocument();
    expect(screen.getByTestId("wecom-byo-bot-id")).toHaveValue("aibot-test");
  });

  it("opens the BYO dialog and submits the trimmed bot id + secret", async () => {
    mockRegisterBYO.mockResolvedValue({ id: "i1", agent_id: "agent-1", status: "installed" });
    renderUI(<WecomAgentBindButton agentId="agent-1" agentName="Bot" />);
    await userEvent.click(screen.getByTestId("wecom-agent-connect"));
    await userEvent.type(await screen.findByTestId("wecom-byo-bot-id"), "  aibot_xyz  ");
    await userEvent.type(screen.getByTestId("wecom-byo-secret"), "  s3cr3t  ");
    await userEvent.click(screen.getByTestId("wecom-byo-submit"));
    await waitFor(() =>
      // Credential trimming: leading/trailing whitespace is stripped before send.
      expect(mockRegisterBYO).toHaveBeenCalledWith("workspace-1", "agent-1", {
        bot_id: "aibot_xyz",
        secret: "s3cr3t",
      }),
    );
  });

  // The whole point of the server sending a code: a zh-Hans admin gets the
  // Chinese sentence, not the English one the API happens to carry.
  it("renders the localized sentence for a coded failure, not the server's English", async () => {
    mockRegisterBYO.mockRejectedValue(
      Object.assign(new Error("this bot is already installed for another agent in this workspace"), {
        code: "wecom_bot_owned_by_same_workspace",
      }),
    );
    renderUI(<WecomAgentBindButton agentId="agent-1" agentName="Bot" />);
    await userEvent.click(screen.getByTestId("wecom-agent-connect"));
    await userEvent.type(await screen.findByTestId("wecom-byo-bot-id"), "aibot_xyz");
    await userEvent.type(screen.getByTestId("wecom-byo-secret"), "s3cr3t");
    await userEvent.click(screen.getByTestId("wecom-byo-submit"));
    await waitFor(() =>
      expect(toast.error).toHaveBeenCalledWith(enSettings.wecom.byo_conflict_same_workspace),
    );
  });

  // The 503 branch: we could not reach WeCom, so nothing was verified and
  // nothing was changed. The admin must not read this as "your secret is
  // wrong" — that sends them to rotate one that was fine, and a rotated WeCom
  // secret cannot be recovered. Separate code, separate sentence.
  it("tells the admin their credentials were not changed when WeCom was unreachable", async () => {
    mockRegisterBYO.mockRejectedValue(
      Object.assign(new Error("could not reach WeCom to verify this bot"), {
        code: "wecom_credentials_unverifiable",
      }),
    );
    renderUI(<WecomAgentBindButton agentId="agent-1" agentName="Bot" />);
    await userEvent.click(screen.getByTestId("wecom-agent-connect"));
    await userEvent.type(await screen.findByTestId("wecom-byo-bot-id"), "aibot_xyz");
    await userEvent.type(screen.getByTestId("wecom-byo-secret"), "s3cr3t");
    await userEvent.click(screen.getByTestId("wecom-byo-submit"));
    await waitFor(() =>
      expect(toast.error).toHaveBeenCalledWith(enSettings.wecom.byo_credentials_unverifiable),
    );
  });

  // WeCom refusing the pair is the one outcome entitled to blame the
  // credentials, and it must not reuse byo_rejected — that sentence means
  // "you left a field out".
  it("renders the credentials-rejected sentence, distinct from the missing-field one", async () => {
    mockRegisterBYO.mockRejectedValue(
      Object.assign(new Error("WeCom rejected this Bot ID and secret"), {
        code: "wecom_credentials_rejected",
      }),
    );
    renderUI(<WecomAgentBindButton agentId="agent-1" agentName="Bot" />);
    await userEvent.click(screen.getByTestId("wecom-agent-connect"));
    await userEvent.type(await screen.findByTestId("wecom-byo-bot-id"), "aibot_xyz");
    await userEvent.type(screen.getByTestId("wecom-byo-secret"), "s3cr3t");
    await userEvent.click(screen.getByTestId("wecom-byo-submit"));
    await waitFor(() =>
      expect(toast.error).toHaveBeenCalledWith(enSettings.wecom.byo_credentials_rejected),
    );
    expect(toast.error).not.toHaveBeenCalledWith(enSettings.wecom.byo_rejected);
  });

  // A server that sends no code must still say something useful, so the
  // fallback cannot regress into the generic toast.
  it("falls back to the server's message when no code is attached", async () => {
    mockRegisterBYO.mockRejectedValue(new Error("something the server said"));
    renderUI(<WecomAgentBindButton agentId="agent-1" agentName="Bot" />);
    await userEvent.click(screen.getByTestId("wecom-agent-connect"));
    await userEvent.type(await screen.findByTestId("wecom-byo-bot-id"), "aibot_xyz");
    await userEvent.type(screen.getByTestId("wecom-byo-secret"), "s3cr3t");
    await userEvent.click(screen.getByTestId("wecom-byo-submit"));
    await waitFor(() =>
      expect(toast.error).toHaveBeenCalledWith("something the server said"),
    );
  });

  // The bot's chat name reaches the server, and only when it was typed.
  //
  // WeCom delivers a group @-mention as literal text with no structured
  // mention list, so a name containing a space ("Orvilo Bot") swallows the
  // slash command typed after it. The name is the only way to tell where the
  // mention ends — and it cannot be discovered, because the smart bot exposes
  // no REST surface to ask. Left blank, the request omits it entirely so a
  // re-install keeps whatever name the row already carries.
  it("sends the bot's chat name when one is given, and omits it when not", async () => {
    mockRegisterBYO.mockResolvedValue({ id: "i-1" });
    renderUI(<WecomAgentBindButton agentId="agent-1" />);
    await userEvent.click(screen.getByTestId("wecom-agent-connect"));
    await userEvent.type(await screen.findByTestId("wecom-byo-bot-id"), "aib94");
    await userEvent.type(screen.getByTestId("wecom-byo-secret"), "s3cret");
    await userEvent.type(screen.getByTestId("wecom-byo-bot-name"), "  Orvilo Bot  ");
    await userEvent.click(screen.getByTestId("wecom-byo-submit"));

    await waitFor(() => expect(mockRegisterBYO).toHaveBeenCalled());
    expect(mockRegisterBYO.mock.calls[0]?.[2].bot_name).toBe("Orvilo Bot");

    cleanup();
    mockRegisterBYO.mockClear();
    mockRegisterBYO.mockResolvedValue({ id: "i-2" });
    renderUI(<WecomAgentBindButton agentId="agent-2" />);
    await userEvent.click(screen.getByTestId("wecom-agent-connect"));
    await userEvent.type(await screen.findByTestId("wecom-byo-bot-id"), "aib95");
    await userEvent.type(screen.getByTestId("wecom-byo-secret"), "s3cret");
    await userEvent.click(screen.getByTestId("wecom-byo-submit"));

    await waitFor(() => expect(mockRegisterBYO).toHaveBeenCalled());
    expect(mockRegisterBYO.mock.calls[0]?.[2].bot_name).toBeUndefined();
  });

  it("shows the installation status (not the CTA) when the agent has an installed bot", () => {
    installationsRef.current = {
      installations: [{ id: "i1", agent_id: "agent-1", bot_id: "aibot_x", status: "installed" }],
      configured: true,
      install_supported: true,
    };
    renderUI(<WecomAgentBindButton agentId="agent-1" />);
    expect(screen.getByTestId("wecom-agent-bot-installed")).toBeTruthy();
    expect(screen.queryByTestId("wecom-agent-connect")).toBeNull();
  });

  it("treats a revoked install as unbound — shows the CTA, not the connected badge", () => {
    installationsRef.current = {
      installations: [{ id: "i1", agent_id: "agent-1", bot_id: "aibot_x", status: "revoked" }],
      configured: true,
      install_supported: true,
    };
    renderUI(<WecomAgentBindButton agentId="agent-1" agentName="Bot" />);
    expect(screen.getByTestId("wecom-agent-connect")).toBeTruthy();
    expect(screen.queryByTestId("wecom-agent-bot-installed")).toBeNull();
  });

  it("renders nothing for a non-manager (member)", () => {
    membersRef.current = [{ user_id: "user-1", role: "member" }];
    const { container } = renderUI(<WecomAgentBindButton agentId="agent-1" />);
    expect(container).toBeEmptyDOMElement();
  });

  it("renders nothing when install is unavailable and the agent is unbound", () => {
    installationsRef.current = { installations: [], configured: true, install_supported: false };
    const { container } = renderUI(<WecomAgentBindButton agentId="agent-1" />);
    expect(container).toBeEmptyDOMElement();
  });
});

describe("WecomTab", () => {
  beforeEach(resetFixtures);

  it("surfaces the not-enabled notice when the deployment has no WeCom key", async () => {
    installationsRef.current = { installations: [], configured: false, install_supported: false };
    renderUI(<WecomTab />, { lobe: true });
    expect(await screen.findByText(/WeCom integration not enabled/i)).toBeTruthy();
  });

  it("shows the empty state when configured but nothing is connected", async () => {
    renderUI(<WecomTab />, { lobe: true });
    expect(await screen.findByText(/No bots installed yet/i)).toBeTruthy();
  });

  it("lists a connected installation with its agent name and a disconnect control", async () => {
    installationsRef.current = {
      installations: [{ id: "i1", agent_id: "agent-7", bot_id: "aibot_x", status: "installed" }],
      configured: true,
      install_supported: true,
    };
    renderUI(<WecomTab />, { lobe: true });
    expect(await screen.findByText("Agent agent-7")).toBeTruthy();
    expect(screen.getByRole("button", { name: /disconnect/i })).toBeTruthy();
  });

  /**
   * The rethrow contract, asserted twice and at two layers, because the
   * DOM-level-only version of this test could not fail.
   *
   * The old assertion here waited for `role="alertdialog"` — the shadcn
   * `AlertDialog`'s role. The imperative confirm is the base-ui `Modal` and
   * carries no such role, so that query was replaced by the dialog's own copy.
   *
   * `confirmModal` closes on the line after `onOk` unless `onOk` returns a
   * promise, and stays open when that promise rejects — so a disconnect that
   * fails must leave the confirmation up rather than dismiss as though the bot
   * were gone. The first test below keeps the DOM spelling but waits a window
   * in which the success path is shown to close; without that demonstration
   * "the title is still in the document" could be true because nothing ever
   * leaves it. The second drops the DOM entirely: the contract lives in the
   * promise `onOk` returns, and that is directly readable off the config the
   * wrapper handed the library.
   */
  it("confirms before disconnecting, and the dialog closes when it succeeds", async () => {
    mockDeleteInstallation.mockResolvedValue(undefined);
    installationsRef.current = {
      installations: [{ id: "i1", agent_id: "agent-7", bot_id: "aibot_x", status: "installed" }],
      configured: true,
      install_supported: true,
    };
    renderUI(<WecomTab />, { lobe: true });
    // The row control only opens the confirm dialog — it must NOT delete yet.
    await userEvent.click(await screen.findByRole("button", { name: /disconnect/i }));
    expect(mockDeleteInstallation).not.toHaveBeenCalled();

    await screen.findByText("Disconnect this WeCom bot?");
    // Two buttons carry this name — the row's and the dialog's — and the
    // dialog's is the last in document order (it is portalled to the end of
    // `<body>`), so `.at(-1)` is the one that runs the request.
    const confirmButtons = await screen.findAllByRole("button", { name: /^Disconnect$/i });
    await userEvent.click(confirmButtons.at(-1)!);
    await waitFor(() =>
      expect(mockDeleteInstallation).toHaveBeenCalledWith("workspace-1", "i1"),
    );
    expect(mockInvalidate).toHaveBeenCalledWith({
      queryKey: ["wecom", "installations", "workspace-1"],
    });
    expect(await dialogLeavesWithin(CLOSE_BUDGET_MS)).toBe(true);
  });

  it("keeps the confirmation up when the disconnect request fails", async () => {
    mockDeleteInstallation.mockRejectedValue(new Error("network failed"));
    installationsRef.current = {
      installations: [{ id: "i1", agent_id: "agent-7", bot_id: "aibot_x", status: "installed" }],
      configured: true,
      install_supported: true,
    };
    renderUI(<WecomTab />, { lobe: true });
    await userEvent.click(await screen.findByRole("button", { name: /disconnect/i }));
    await screen.findByText("Disconnect this WeCom bot?");
    const confirmButtons = await screen.findAllByRole("button", { name: /^Disconnect$/i });
    await userEvent.click(confirmButtons.at(-1)!);

    await waitFor(() => expect(toast.error).toHaveBeenCalledWith("network failed"));
    expect(await dialogLeavesWithin(CLOSE_WINDOW_MS)).toBe(false);
  });

  it("hands confirmModal an onOk that rejects when the disconnect fails", async () => {
    mockDeleteInstallation.mockRejectedValue(new Error("network failed"));
    installationsRef.current = {
      installations: [{ id: "i1", agent_id: "agent-7", bot_id: "aibot_x", status: "installed" }],
      configured: true,
      install_supported: true,
    };
    renderUI(<WecomTab />, { lobe: true });
    await userEvent.click(await screen.findByRole("button", { name: /disconnect/i }));
    expect(capturedConfirm.current).not.toBeNull();

    await expect(capturedConfirm.current?.onOk()).rejects.toThrow("network failed");
    expect(toast.error).toHaveBeenCalledWith("network failed");
  });
});
