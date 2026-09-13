// @vitest-environment jsdom

import { type ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { configStore } from "@orvilo/core/config";
import enSettings from "../../locales/en/settings.json";

type MemberRole = "owner" | "admin" | "member" | "guest";

const membersRef = vi.hoisted(() => ({
  current: [{ user_id: "user-1", role: "owner" as MemberRole }] as unknown[],
}));
const agentsRef = vi.hoisted(() => ({
  current: [
    { id: "agent-1", name: "Planner", owner_id: "user-1", archived_at: null },
    { id: "agent-archived", name: "Archived", owner_id: "user-1", archived_at: "2026-01-01" },
  ] as unknown[],
}));
const installationsRef = vi.hoisted(() => ({
  current: {
    installations: [] as unknown[],
    configured: true,
    install_supported: true,
  },
}));
const installationsLoadingRef = vi.hoisted(() => ({ current: false }));
const installationsErrorRef = vi.hoisted(() => ({ current: false }));
const mockBegin = vi.hoisted(() => vi.fn());
const mockStatus = vi.hoisted(() => vi.fn());
const mockDelete = vi.hoisted(() => vi.fn());
const mockInvalidate = vi.hoisted(() => vi.fn());
const mockToastSuccess = vi.hoisted(() => vi.fn());
const mockToastError = vi.hoisted(() => vi.fn());
const MockApiError = vi.hoisted(() =>
  class MockApiError extends Error {
    readonly status: number;

    constructor(message: string, status: number) {
      super(message);
      this.name = "ApiError";
      this.status = status;
    }
  },
);

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
    if (opts.enabled === false) return { data: undefined, isLoading: false, isError: false };
    const key = JSON.stringify(opts.queryKey);
    if (key.includes("members")) return { data: membersRef.current, isLoading: false, isError: false };
    if (key.includes("agents")) return { data: agentsRef.current, isLoading: false, isError: false };
    if (key.includes("weixin")) {
      return {
        data: installationsLoadingRef.current ? undefined : installationsRef.current,
        isLoading: installationsLoadingRef.current,
        isError: installationsErrorRef.current,
      };
    }
    return { data: undefined, isLoading: false, isError: false };
  },
  useQueryClient: () => ({ invalidateQueries: mockInvalidate }),
  queryOptions: <T,>(opts: T) => opts,
}));

vi.mock("@orvilo/core/hooks", () => ({ useWorkspaceId: () => "workspace-1" }));

vi.mock("@orvilo/core/workspace/queries", () => ({
  memberListOptions: () => ({ queryKey: ["members"], queryFn: vi.fn() }),
  agentListOptions: () => ({ queryKey: ["agents"], queryFn: vi.fn() }),
}));

vi.mock("@orvilo/core/weixin", () => ({
  weixinInstallationsOptions: () => ({
    queryKey: ["weixin", "workspace-1", "installations"],
    queryFn: vi.fn(),
  }),
  weixinKeys: {
    installations: (wsId: string) => ["weixin", wsId, "installations"],
  },
}));

vi.mock("@orvilo/core/api", () => ({
  api: {
    beginWeixinInstall: mockBegin,
    getWeixinInstallStatus: mockStatus,
    deleteWeixinInstallation: mockDelete,
  },
  ApiError: MockApiError,
}));

vi.mock("@orvilo/core/auth", () => {
  const useAuthStore = Object.assign(
    (selector?: (state: { user: { id: string } }) => unknown) =>
      selector ? selector({ user: { id: "user-1" } }) : { user: { id: "user-1" } },
    { getState: () => ({ user: { id: "user-1" } }) },
  );
  return { useAuthStore };
});

vi.mock("react-qr-code", () => ({
  QRCode: ({ value }: { value: string }) => <svg data-testid="weixin-qr" data-value={value} />,
}));

vi.mock("sonner", () => ({
  toast: { success: mockToastSuccess, error: mockToastError, message: vi.fn() },
}));

import { renderWithI18n } from "../../test/i18n";
import {
  expectLobePrimaryTreatment,
  lobeButtonTreatmentClasses,
} from "../../test/lobe-button-treatment";
import { WeixinAgentBindButton, WeixinTab } from "./weixin-tab";

afterEach(cleanup);

/**
 * `lobe: true` mounts the theme bridge on demand, so everything after a
 * bridged render must start with an async query — until the bridge's module
 * resolves the tree is a `Suspense` fallback of `null` and a synchronous
 * `getBy*` would fail.
 *
 * The flag defaults to **off** here rather than on, which is the opposite of
 * the Slack/Telegram suite's helper, and deliberately so: most of this file
 * renders `WeixinAgentBindButton`, which renders no Lobe element at all. A
 * bridged render would make the negative assertions in those tests pass
 * *vacuously* — the container is empty while the bridge module resolves — which
 * is the "guard that cannot fail" shape. Renders that can reach a migrated
 * component pass `lobe: true` explicitly, and a render that reaches one without
 * the bridge throws loudly rather than failing silently.
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
      () => expect(screen.queryByText("Disconnect this Weixin account?")).toBeNull(),
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

describe("Weixin deployment setup policy", () => {
  beforeEach(resetFixtures);

  it.each(["server_configured", "disabled"] as const)("keeps %s credential lifecycle read-only", async (mode) => {
    configStore.getState().setMessagingConfig({ mode, setupWritable: true, platforms: [] });
    // No bridge: on this path the component renders `MessagingSetupNotice`
    // (shadcn) or `null`, so no Lobe element renders — and the negative
    // assertion below is decided by the component rather than by a bridge
    // module that has not resolved yet.
    const entry = renderUI(<WeixinAgentBindButton agentId="agent-1" />, { lobe: false });
    expect(screen.queryByRole("button", { name: "Connect Weixin" })).toBeNull();
    expect(mockBegin).not.toHaveBeenCalled();
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
    renderUI(<><WeixinAgentBindButton agentId="agent-1" /><WeixinTab /></>, { lobe: true });
    expect(await screen.findAllByRole("status", { name: "Connection status" })).toHaveLength(2);
    expect(screen.queryByRole("button", { name: /disconnect/i })).toBeNull();
    expect(mockDelete).not.toHaveBeenCalled();
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
  agentsRef.current = [
    { id: "agent-1", name: "Planner", owner_id: "user-1", archived_at: null },
    { id: "agent-archived", name: "Archived", owner_id: "user-1", archived_at: "2026-01-01" },
  ];
  installationsRef.current = { installations: [], configured: true, install_supported: true };
  installationsLoadingRef.current = false;
  installationsErrorRef.current = false;
  mockInvalidate.mockResolvedValue(undefined);
}

describe("WeixinTab", () => {
  beforeEach(resetFixtures);

  it("starts workspace QR setup from the shared App connect entry", async () => {
    mockBegin.mockResolvedValue({
      session_id: "workspace-session",
      qr_code_url: "weixin://qr/workspace-1",
      expires_in_seconds: 300,
      poll_interval_seconds: 2,
    });
    renderUI(<WeixinAgentBindButton />);
    await userEvent.click(screen.getByRole("button", { name: "Connect Weixin" }));
    await waitFor(() => expect(mockBegin).toHaveBeenCalledWith("workspace-1", undefined));
  });

  it("does not expose direct setup to a member who does not own the Agent", () => {
    membersRef.current = [{ user_id: "user-1", role: "member" }];
    agentsRef.current = [{ id: "agent-1", name: "Planner", owner_id: "user-2", archived_at: null }];
    renderUI(<WeixinAgentBindButton agentId="agent-1" />);
    expect(screen.queryByRole("button", { name: "Connect Weixin" })).toBeNull();
    expect(mockBegin).not.toHaveBeenCalled();
  });

  /**
   * Where the primary treatment lands — asserted as *treatment*, not presence,
   * and not props.
   *
   * The connect CTA shipped as a secondary control. Its base was a shadcn
   * `<Button>` with **no `variant`** (`bg-primary text-primary-foreground`, the
   * loudest control in the row), and the migrated `<LobeButton>` had **no
   * `type`**, which is Lobe's outlined default: the two libraries' defaults point
   * opposite ways, so the call site read identically before and after and no prop
   * diff could see the demotion. `01f54227` restored `type="primary"`.
   *
   * Both halves are asserted in one render on purpose. "The CTA is primary" alone
   * would also pass on a build where every button in the row is primary — a
   * guard asserting a treatment that nothing distinguishes. The row's Disconnect
   * is the control: its base was `<Button variant="outline">`, Lobe's default
   * already is that treatment, and promoting it to primary would be a new defect
   * rather than a restoration.
   *
   * What this is **not**: it pins that each control wears the class Lobe paints
   * for that treatment, not what the treatment looks like. The pixel pass is
   * still what says the two are the same colour.
   */
  it("keeps the connect CTA primary and the row's Disconnect outlined", async () => {
    // Two agents, so one row can hold an installation and the other the CTA that
    // creates one — `availableAgents` excludes an agent that is already
    // installed, so a single agent cannot show both controls.
    agentsRef.current = [
      { id: "agent-1", name: "Planner", owner_id: "user-1", archived_at: null },
      { id: "agent-2", name: "Researcher", owner_id: "user-1", archived_at: null },
    ];
    installationsRef.current = {
      installations: [{ id: "installation-1", agent_id: "agent-1", bot_id: "personal-bot", status: "installed" }],
      configured: true,
      install_supported: true,
    };
    renderUI(<WeixinTab />, { lobe: true });

    await expectLobePrimaryTreatment(await screen.findByTestId("weixin-connect-agent-agent-2"));

    const { primary } = await lobeButtonTreatmentClasses();
    expect(await screen.findByRole("button", { name: /^Disconnect$/i })).not.toHaveClass(primary);
  });

  it("starts a Personal Weixin session and renders its QR code", async () => {
    mockBegin.mockResolvedValue({
      session_id: "session-1",
      qr_code_url: "weixin://qr/personal-1",
      expires_in_seconds: 300,
      poll_interval_seconds: 30,
    });
    const user = userEvent.setup();
    renderUI(<WeixinTab />, { lobe: true });

    await user.click(await screen.findByTestId("weixin-connect-agent-agent-1"));
    await waitFor(() => expect(mockBegin).toHaveBeenCalledWith("workspace-1", "agent-1"));
    expect(await screen.findByTestId("weixin-qr")).toHaveAttribute(
      "data-value",
      "weixin://qr/personal-1",
    );
    expect(screen.getByRole("link", { name: /authorization link/i })).toHaveAttribute(
      "href",
      "weixin://qr/personal-1",
    );
  });

  it("polls for the verification-code state and redeems the trimmed code", async () => {
    mockBegin.mockResolvedValue({
      session_id: "session-verify",
      qr_code_url: "weixin://qr/verify",
      expires_in_seconds: 300,
      poll_interval_seconds: 0,
    });
    mockStatus
      .mockResolvedValueOnce({ status: "need_verify_code" })
      .mockResolvedValueOnce({ status: "success", installation_id: "installation-1" });
    const user = userEvent.setup();
    renderUI(<WeixinTab />, { lobe: true });

    await user.click(await screen.findByTestId("weixin-connect-agent-agent-1"));
    const codeInput = await screen.findByTestId("weixin-verify-code", {}, { timeout: 4000 });
    await user.type(codeInput, " 2468 ");
    await user.click(screen.getByRole("button", { name: /continue/i }));

    await waitFor(() =>
      expect(mockStatus).toHaveBeenLastCalledWith("workspace-1", "session-verify", "2468"),
    );
    await waitFor(() => expect(mockInvalidate).toHaveBeenCalledWith({
      queryKey: ["weixin", "workspace-1", "installations"],
    }));
    expect(mockToastSuccess).toHaveBeenCalled();
    // No per-test timeout here, deliberately. It used to carry `, 7000`, and
    // that override was the whole of its flakiness: once this tab rendered
    // through Lobe (`{ lobe: true }` above), its cost rose to where the rest of
    // the file sits -- measured in isolation, this test 5.6-7.0s and its
    // sibling "keeps the confirmation up" 6.7s, against the suite's 20s budget.
    // The sibling passes because it inherits that budget; this one was pinned to
    // 7s and sat at ~99% of it, so a CI runner crossed it. The suite's 20s is
    // itself the repository's answer to antd's CSS-in-JS cost under jsdom (see
    // the `testTimeout` comment in `packages/views/vitest.config.ts`), so
    // inheriting is the consistent value -- and the 7s was never an assertion
    // about bounded time, only a safety net. Do not re-add it.
  });

  it("revokes an installed installation only after confirmation", async () => {
    installationsRef.current = {
      installations: [{ id: "installation-1", agent_id: "agent-1", bot_id: "personal-bot", status: "installed" }],
      configured: true,
      install_supported: true,
    };
    mockDelete.mockResolvedValue(undefined);
    const user = userEvent.setup();
    renderUI(<WeixinTab />, { lobe: true });

    await user.click(await screen.findByRole("button", { name: /^Disconnect$/i }));
    expect(mockDelete).not.toHaveBeenCalled();
    // Two buttons carry this name — the row's and the dialog's — and the
    // dialog's is the last in document order (it is portalled to the end of
    // `<body>`), so `.at(-1)` is the one that runs the request.
    const confirmButtons = await screen.findAllByRole("button", { name: /^Disconnect$/i });
    await user.click(confirmButtons.at(-1)!);
    await waitFor(() =>
      expect(mockDelete).toHaveBeenCalledWith("workspace-1", "installation-1"),
    );
    expect(mockInvalidate).toHaveBeenCalledWith({
      queryKey: ["weixin", "workspace-1", "installations"],
    });
    expect(await dialogLeavesWithin(CLOSE_BUDGET_MS)).toBe(true);
  });

  it("renders unknown installation statuses as a safe unavailable fallback", async () => {
    installationsRef.current = {
      installations: [{ id: "installation-unknown", agent_id: "agent-1", bot_id: "bot", status: "future_status" }],
      configured: true,
      install_supported: true,
    };
    renderUI(<WeixinTab />, { lobe: true });

    expect(await screen.findByText(/^revoked$/i)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /^Disconnect$/i })).toBeNull();
  });

  /**
   * The caller's half of the rethrow contract, at both observable layers.
   *
   * `confirmModal` closes on the line after `onOk` unless `onOk` returns a
   * promise, and stays open when that promise rejects — so a disconnect that
   * fails must leave the confirmation up rather than dismiss as though the
   * account were gone. The success path above is what gives the negative
   * assertion meaning: without a run in which the dialog is shown to close, "it
   * is still in the document" could be true because nothing ever leaves it. The
   * second test here drops the DOM entirely — the contract lives in the promise
   * `onOk` returns, and that is directly readable off the config the wrapper
   * handed the library.
   */
  it("keeps the confirmation up when the disconnect request fails", async () => {
    mockDelete.mockRejectedValue(new Error("network failed"));
    installationsRef.current = {
      installations: [{ id: "installation-1", agent_id: "agent-1", bot_id: "personal-bot", status: "installed" }],
      configured: true,
      install_supported: true,
    };
    const user = userEvent.setup();
    renderUI(<WeixinTab />, { lobe: true });

    await user.click(await screen.findByRole("button", { name: /^Disconnect$/i }));
    await screen.findByText("Disconnect this Weixin account?");
    const confirmButtons = await screen.findAllByRole("button", { name: /^Disconnect$/i });
    await user.click(confirmButtons.at(-1)!);

    await waitFor(() => expect(mockToastError).toHaveBeenCalledWith(enSettings.weixin.toast_disconnect_failed));
    expect(await dialogLeavesWithin(CLOSE_WINDOW_MS)).toBe(false);
  });

  it("hands confirmModal an onOk that rejects when the disconnect fails", async () => {
    mockDelete.mockRejectedValue(new Error("network failed"));
    installationsRef.current = {
      installations: [{ id: "installation-1", agent_id: "agent-1", bot_id: "personal-bot", status: "installed" }],
      configured: true,
      install_supported: true,
    };
    const user = userEvent.setup();
    renderUI(<WeixinTab />, { lobe: true });

    await user.click(await screen.findByRole("button", { name: /^Disconnect$/i }));
    expect(capturedConfirm.current).not.toBeNull();

    await expect(capturedConfirm.current?.onOk()).rejects.toThrow("network failed");
    expect(mockToastError).toHaveBeenCalledWith(enSettings.weixin.toast_disconnect_failed);
  });
});
