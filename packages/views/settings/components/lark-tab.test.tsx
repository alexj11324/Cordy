import { StrictMode, type ReactNode } from "react";
import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { act, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { configStore } from "@orvilo/core/config";

// ApiError is re-exported from @orvilo/core/api; we mock the api module
// itself but still need a real ApiError class so `e instanceof ApiError`
// in the polling catch behaves the way it does at runtime.
const ApiError = vi.hoisted(() => {
  class ApiError extends Error {
    readonly status: number;
    readonly statusText: string;
    readonly body?: unknown;
    constructor(message: string, status: number, statusText = "", body?: unknown) {
      super(message);
      this.name = "ApiError";
      this.status = status;
      this.statusText = statusText;
      this.body = body;
    }
  }
  return ApiError;
});

const mockBeginInstall = vi.hoisted(() => vi.fn());
const mockGetStatus = vi.hoisted(() => vi.fn());
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

vi.mock("@tanstack/react-query", () => ({
  useQuery: (opts: { queryKey: unknown[]; enabled?: boolean }) => {
    if (opts.enabled === false) return { data: undefined, isLoading: false };
    const key = JSON.stringify(opts.queryKey);
    if (key.includes("members")) return { data: membersRef.current, isLoading: false };
    if (key.includes("installations")) {
      return { data: installationsRef.current, isLoading: false };
    }
    return { data: undefined, isLoading: false };
  },
  useQueryClient: () => ({
    invalidateQueries: mockInvalidate,
  }),
  queryOptions: <T,>(opts: T) => opts,
}));

vi.mock("@orvilo/core/hooks", () => ({
  useWorkspaceId: () => "workspace-1",
}));

vi.mock("@orvilo/core/workspace/queries", () => ({
  memberListOptions: () => ({ queryKey: ["members"], queryFn: vi.fn() }),
}));

// useActorName is the workspace-wide identity helper. The Installation
// row uses it to render the Orvilo agent's name in place of the raw
// Lark app_id. Stubbing it here decouples LarkTab tests from the agent
// list query plumbing.
const agentNameByIdRef = vi.hoisted(() => ({
  current: new Map<string, string>(),
}));
vi.mock("@orvilo/core/workspace/hooks", () => ({
  useActorName: () => ({
    getAgentName: (agentId: string) =>
      agentNameByIdRef.current.get(agentId) ?? "Unknown Agent",
    getMemberName: () => "Unknown",
    getTeamName: () => "Unknown Team",
    getActorName: () => "Unknown",
    getActorInitials: () => "??",
    getActorAvatarUrl: () => null,
  }),
}));

// ActorAvatar pulls in a deep tree (hover cards, presence query, paths).
// In LarkTab tests we only care that the row identifies the correct
// agent — render a tiny stub that surfaces actorId in the DOM so the
// agent-identity assertion can read it directly.
vi.mock("../../common/actor-avatar", () => ({
  ActorAvatar: ({ actorType, actorId }: { actorType: string; actorId: string }) => (
    <span data-testid="actor-avatar" data-actor-type={actorType} data-actor-id={actorId} />
  ),
}));

vi.mock("@orvilo/core/lark", () => ({
  larkInstallationsOptions: () => ({
    queryKey: ["lark", "installations"],
    queryFn: vi.fn(),
  }),
  larkKeys: { installations: (wsId: string) => ["lark", "installations", wsId] },
}));

vi.mock("@orvilo/core/api", () => ({
  api: {
    beginLarkInstall: mockBeginInstall,
    getLarkInstallStatus: mockGetStatus,
    deleteLarkInstallation: mockDeleteInstallation,
  },
  ApiError,
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
  toast: {
    success: vi.fn(),
    error: vi.fn(),
    message: vi.fn(),
  },
}));

// react-qr-code renders SVG that jsdom doesn't fully support — a stub
// keeps the dialog DOM compact and lets us assert on the surrounding
// chrome (status text, buttons) without QR mechanics.
// Expose the stub as BOTH the named `QRCode` export (what lark-tab now
// imports — see the named-import interop fix) and `default`, so the mock
// stays correct regardless of how the component pulls it in. The stub is
// defined inside the factory because vi.mock is hoisted above any
// top-level variable.
vi.mock("react-qr-code", () => {
  const QrStub = ({ value }: { value: string }) => (
    <span data-testid="qr-code" data-value={value} />
  );
  return { QRCode: QrStub, default: QrStub };
});

import { renderWithI18n } from "../../test/i18n";
import { LarkAgentBindButton, LarkTab } from "./lark-tab";
import { toast } from "sonner";

/**
 * `lobe: true` mounts the theme bridge on demand, so everything after a
 * bridged render must start with an async query — until the bridge's module
 * resolves the tree is a `Suspense` fallback of `null` and a synchronous
 * `getBy*` would fail.
 *
 * The flag defaults to **off** here rather than on, which is the opposite of
 * the Slack/Telegram suite's helper, and deliberately so: most of this file
 * renders `LarkAgentBindButton` and its sub-components, which render no Lobe
 * element at all. A bridged render would make the three "renders nothing"
 * assertions below pass *vacuously* — the container is empty while the bridge
 * module resolves — which is the "guard that cannot fail" shape the reference
 * names. Renders that can reach a migrated component pass `lobe: true`
 * explicitly, and a render that reaches one without the bridge throws loudly
 * rather than failing silently.
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
      () => expect(screen.queryByText("Disconnect this Lark bot?")).toBeNull(),
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

describe("Lark deployment setup policy", () => {
  beforeEach(resetFixtures);

  it.each(["server_configured", "disabled"] as const)("keeps %s credential lifecycle read-only", async (mode) => {
    configStore.getState().setMessagingConfig({ mode, setupWritable: true, platforms: [] });
    const entry = renderUI(<LarkAgentBindButton agentId="agent-1" />, { lobe: false });
    expect(screen.queryByTestId("lark-agent-bind-feishu")).toBeNull();
    expect(mockBeginInstall).not.toHaveBeenCalled();
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
    renderUI(<><LarkAgentBindButton agentId="agent-1" /><LarkTab /></>, { lobe: true });
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
  installationsRef.current = {
    installations: [],
    configured: true,
    install_supported: true,
  };
  agentNameByIdRef.current = new Map();
}

describe("LarkAgentBindButton (CTA gate)", () => {
  beforeEach(resetFixtures);

  it("shows the Feishu bind CTA but hides the Lark CTA for an owner (MUL-3083)", () => {
    // Mainland Feishu binding stays available; the Lark (international)
    // entry is temporarily hidden via LARK_INTL_CONNECT_ENABLED while its
    // install→inbound pipeline is stabilized (MUL-3083).
    renderUI(<LarkAgentBindButton agentId="agent-1" agentName="Bot" />);
    expect(screen.getByRole("button", { name: /Bind to Feishu/i })).toBeTruthy();
    expect(screen.queryByRole("button", { name: /Bind to Lark/i })).toBeNull();
  });

  it("shows the Feishu bind CTA but hides the Lark CTA for an admin (MUL-3083)", () => {
    membersRef.current = [{ user_id: "user-1", role: "admin" }];
    renderUI(<LarkAgentBindButton agentId="agent-1" agentName="Bot" />);
    expect(screen.getByRole("button", { name: /Bind to Feishu/i })).toBeTruthy();
    expect(screen.queryByRole("button", { name: /Bind to Lark/i })).toBeNull();
  });

  it("hides both bind CTAs for a plain member when no agentOwnerId is supplied (stays admin-only)", () => {
    membersRef.current = [{ user_id: "user-1", role: "member" }];
    const { container } = renderUI(
      <LarkAgentBindButton agentId="agent-1" agentName="Bot" />,
    );
    expect(container.querySelector("button")).toBeNull();
  });

  it("shows the Feishu bind CTA for a non-admin agent owner (agentOwnerId matches the user, MUL-4213)", () => {
    // The backend authorizes the agent's owner via canManageAgent even when
    // they are only a plain workspace member, so the CTA must render for
    // them once the caller threads the agent's owner_id.
    membersRef.current = [{ user_id: "user-1", role: "member" }];
    renderUI(
      <LarkAgentBindButton
        agentId="agent-1"
        agentName="Bot"
        agentOwnerId="user-1"
      />,
    );
    expect(screen.getByRole("button", { name: /Bind to Feishu/i })).toBeTruthy();
  });

  it("still hides the CTAs for a non-admin member who is NOT the agent owner", () => {
    // agentOwnerId belongs to someone else, so a plain member gets nothing.
    membersRef.current = [{ user_id: "user-1", role: "member" }];
    const { container } = renderUI(
      <LarkAgentBindButton
        agentId="agent-1"
        agentName="Bot"
        agentOwnerId="user-2"
      />,
    );
    expect(container.querySelector("button")).toBeNull();
  });

  it("hides both bind CTAs when the device-flow install path is not wired on the server", () => {
    installationsRef.current.install_supported = false;
    const { container } = renderUI(
      <LarkAgentBindButton agentId="agent-1" agentName="Bot" />,
    );
    expect(container.querySelector("button")).toBeNull();
  });

  it("clicking Bind to Feishu begins an install with region='feishu'", async () => {
    // Pin the routing wire-up: each split CTA must pass its own region
    // string to the API client (which threads it onto the
    // /lark/install/begin?region=… query param), so the device-flow
    // begins on the matching accounts host. A regression here would
    // silently send Lark users to a Feishu QR — the exact bug this
    // refactor addresses.
    const user = userEvent.setup();
    mockBeginInstall.mockResolvedValue({
      session_id: "sess-feishu",
      qr_code_url: "https://accounts.feishu.cn/oauth/v1/device?u=feishu",
      expires_in_seconds: 300,
      poll_interval_seconds: 2,
    });
    mockGetStatus.mockResolvedValue({ status: "pending" });
    renderUI(<LarkAgentBindButton agentId="agent-1" agentName="Bot" />);
    await user.click(screen.getByRole("button", { name: /Bind to Feishu/i }));
    await waitFor(() => {
      expect(mockBeginInstall).toHaveBeenCalledTimes(1);
    });
    expect(mockBeginInstall).toHaveBeenCalledWith(
      "workspace-1",
      "agent-1",
      "feishu",
    );
  });

  // NOTE (MUL-3083): the "clicking Bind to Lark begins an install with
  // region='lark'" test was removed alongside the temporarily-hidden Lark
  // (international) CTA — there is no Lark button to click while
  // LARK_INTL_CONNECT_ENABLED is false. The Feishu region routing is still
  // pinned by the "clicking Bind to Feishu …" test above; restore the Lark
  // case when the entry is re-enabled.

  it("swaps the bind CTAs for a 'Connected + Manage in Lark' badge when this agent already has an installed installation", () => {
    // Anti-zombie guard: re-scanning the same agent upserts the row
    // and orphans the previously-created Lark PersonalAgent. The badge
    // closes the install entry point and links the user to the Bot's
    // dev console page where scopes / display name / additional
    // permissions are actually managed.
    installationsRef.current.installations = [
      {
        id: "inst-1",
        workspace_id: "ws-1",
        agent_id: "agent-1",
        app_id: "cli_existing_app",
        bot_open_id: "ou_existing_bot",
        installer_user_id: "user-1",
        status: "installed",
        installed_at: "2026-06-03T00:00:00Z",
        created_at: "2026-06-03T00:00:00Z",
        updated_at: "2026-06-03T00:00:00Z",
      },
    ];
    renderUI(
      <LarkAgentBindButton agentId="agent-1" agentName="Bot" />,
    );
    // Both Bind CTAs must be gone — re-scanning would orphan the
    // PersonalAgent (see badge comment in lark-tab.tsx).
    expect(screen.queryByRole("button", { name: /Bind to Feishu/i })).toBeNull();
    expect(screen.queryByRole("button", { name: /Bind to Lark/i })).toBeNull();
    // The fixture omits `region`, which the listings DTO defaults to
    // Feishu (mainland). After the #3830 badge restructure the cloud is
    // shown as a "Feishu" chip (not baked into the connected label) and a
    // Disconnect action appears; the region-aware Manage link still points
    // at the mainland host.
    expect(screen.getByText("Feishu")).toBeTruthy();
    expect(screen.getByTestId("lark-agent-bot-disconnect")).toBeTruthy();
    const link = screen.getByRole("link", { name: /Manage in Feishu/i }) as HTMLAnchorElement;
    expect(link.href).toBe("https://open.feishu.cn/app/cli_existing_app");
    expect(link.target).toBe("_blank");
    expect(link.rel).toContain("noopener");
  });

  it("renders region-aware badge text and Manage link for a Lark-international (region=lark) installation", () => {
    // Dual-region: a bot installed against the Lark international cloud
    // must preserve the Lark region and management link without treating an
    // installed installation as a confirmed live connection. The
    // Manage link pointing at open.larksuite.com (not the Feishu
    // default). Without region-aware copy a user who clicked
    // "Bind to Feishu" and saw "Connected to Lark" would (rightly) be
    // confused — the labels must match the cloud the bot lives on.
    installationsRef.current.installations = [
      {
        id: "inst-lark",
        workspace_id: "ws-1",
        agent_id: "agent-1",
        app_id: "cli_lark_app",
        bot_open_id: "ou_lark_bot",
        installer_user_id: "user-1",
        status: "installed",
        region: "lark",
        installed_at: "2026-06-03T00:00:00Z",
        created_at: "2026-06-03T00:00:00Z",
        updated_at: "2026-06-03T00:00:00Z",
      },
    ];
    renderUI(<LarkAgentBindButton agentId="agent-1" agentName="Bot" />);
    expect(screen.getByRole("status", { name: "Connection status" }).textContent).toBe("Status unavailable");
    expect(screen.getByText("Lark", { exact: true })).toBeTruthy();
    const link = screen.getByRole("link", { name: /Manage in Lark/i }) as HTMLAnchorElement;
    expect(link.href).toBe("https://open.larksuite.com/app/cli_lark_app");
  });

  it("shows the Feishu CTA (Lark hidden) for an agent without its own installation, per-agent scoping (MUL-3083)", () => {
    installationsRef.current.installations = [
      {
        id: "inst-other",
        workspace_id: "ws-1",
        agent_id: "agent-other",
        app_id: "cli_other",
        bot_open_id: "ou_other",
        installer_user_id: "user-1",
        status: "installed",
        installed_at: "2026-06-03T00:00:00Z",
        created_at: "2026-06-03T00:00:00Z",
        updated_at: "2026-06-03T00:00:00Z",
      },
    ];
    renderUI(<LarkAgentBindButton agentId="agent-1" agentName="Bot" />);
    expect(screen.getByRole("button", { name: /Bind to Feishu/i })).toBeTruthy();
    expect(screen.queryByRole("button", { name: /Bind to Lark/i })).toBeNull();
  });

  it("keeps the Connected + Manage badge for an already-installed agent even when new installs are unavailable (install_supported=false)", () => {
    // install_supported governs only NEW scan-installs — an already-installed
    // bot stays manageable when the device-flow transport is unwired
    // (server/internal/handler/lark.go: "already-installed bots still appear
    // and remain manageable"). Regression: the install_supported gate used to
    // run before the existing-installation check and hid the bound state.
    installationsRef.current.install_supported = false;
    installationsRef.current.installations = [
      {
        id: "inst-1",
        workspace_id: "ws-1",
        agent_id: "agent-1",
        app_id: "cli_existing_app",
        bot_open_id: "ou_existing_bot",
        installer_user_id: "user-1",
        status: "installed",
        installed_at: "2026-06-03T00:00:00Z",
        created_at: "2026-06-03T00:00:00Z",
        updated_at: "2026-06-03T00:00:00Z",
      },
    ];
    renderUI(
      <LarkAgentBindButton agentId="agent-1" agentName="Bot" />,
    );
    // Both Bind CTAs must be gone even when install_supported=false,
    // since the existing-installation check runs first.
    expect(screen.queryByRole("button", { name: /Bind to Feishu/i })).toBeNull();
    expect(screen.queryByRole("button", { name: /Bind to Lark/i })).toBeNull();
    // Fixture omits region → defaults to Feishu: the cloud shows as a
    // "Feishu" chip (post-#3830 badge restructure), the Disconnect action
    // is present, and the Manage link stays Feishu-aware.
    expect(screen.getByText("Feishu")).toBeTruthy();
    expect(screen.getByTestId("lark-agent-bot-disconnect")).toBeTruthy();
    expect(
      screen.getByRole("link", { name: /Manage in Feishu/i }),
    ).toBeTruthy();
  });

  it("shows the Feishu CTA (Lark hidden) when this agent's only installation is revoked (MUL-3083)", () => {
    installationsRef.current.installations = [
      {
        id: "inst-revoked",
        workspace_id: "ws-1",
        agent_id: "agent-1",
        app_id: "cli_revoked",
        bot_open_id: "ou_revoked",
        installer_user_id: "user-1",
        status: "revoked",
        installed_at: "2026-06-03T00:00:00Z",
        created_at: "2026-06-03T00:00:00Z",
        updated_at: "2026-06-03T00:00:00Z",
      },
    ];
    renderUI(<LarkAgentBindButton agentId="agent-1" agentName="Bot" />);
    expect(screen.getByRole("button", { name: /Bind to Feishu/i })).toBeTruthy();
    expect(screen.queryByRole("button", { name: /Bind to Lark/i })).toBeNull();
  });
});

// The Connected badge surfaces an Unbind affordance for owners/admins
// (parent gate keeps non-admins out of this component entirely). The
// disconnect path is the recovery handle for the install_supported=false
// re-scan zombie-bot trap and the dual-bot conflict — these tests pin
// the contract: confirm gating, deleteLarkInstallation wiring, cache
// invalidation, and toast feedback on success / failure.
describe("LarkAgentBotInstalledControls (Unbind / Disconnect)", () => {
  beforeEach(() => {
    resetFixtures();
    installationsRef.current.installations = [
      {
        id: "inst-1",
        workspace_id: "ws-1",
        agent_id: "agent-1",
        app_id: "cli_existing_app",
        bot_open_id: "ou_existing_bot",
        installer_user_id: "user-1",
        status: "installed",
        installed_at: "2026-06-03T00:00:00Z",
        created_at: "2026-06-03T00:00:00Z",
        updated_at: "2026-06-03T00:00:00Z",
      },
    ];
  });

  it("renders a Disconnect affordance alongside the Manage link when the agent is bound", () => {
    renderUI(<LarkAgentBindButton agentId="agent-1" agentName="Bot" />);
    // The badge surfaces three siblings: the green-dot status pill,
    // the Manage link, and the Unbind action. We assert by test-id so
    // we don't trip over /Disconnect/i copy that also appears in the
    // (closed) AlertDialog.
    expect(screen.getByTestId("lark-agent-bot-disconnect")).toBeTruthy();
    // Fixture omits region → Feishu copy.
    expect(screen.getByRole("link", { name: /Manage in Feishu/i })).toBeTruthy();
  });

  it("opens the confirm dialog and does NOT call the API until the user confirms", async () => {
    const user = userEvent.setup();
    renderUI(<LarkAgentBindButton agentId="agent-1" agentName="Bot" />);
    await user.click(screen.getByTestId("lark-agent-bot-disconnect"));
    // Confirm dialog must mount with the correct copy.
    await waitFor(() => {
      expect(
        screen.getByText(/Disconnect this Lark bot\?/i),
      ).toBeTruthy();
    });
    // Critically: clicking the trigger alone must NOT have deleted the
    // installation — confirmation is mandatory.
    expect(mockDeleteInstallation).not.toHaveBeenCalled();
  });

  it("calls deleteLarkInstallation with (workspaceId, installationId), invalidates the cache, and toasts on confirm", async () => {
    mockDeleteInstallation.mockResolvedValue(undefined);
    const user = userEvent.setup();
    renderUI(<LarkAgentBindButton agentId="agent-1" agentName="Bot" />);
    await user.click(screen.getByTestId("lark-agent-bot-disconnect"));
    // Wait for the dialog to mount, then click the destructive action
    // (the AlertDialogAction's accessible name is the same "Disconnect"
    // label as the trigger button — but we're now inside the dialog
    // role, so role+name is unambiguous).
    const confirmButton = await screen.findByRole("button", {
      name: /^Disconnect$/i,
    });
    await user.click(confirmButton);

    await waitFor(() => {
      expect(mockDeleteInstallation).toHaveBeenCalledTimes(1);
    });
    expect(mockDeleteInstallation).toHaveBeenCalledWith("workspace-1", "inst-1");
    // Listings cache must be invalidated so the parent re-renders the
    // Bind CTA in place of the now-stale Connected badge.
    expect(mockInvalidate).toHaveBeenCalledWith({
      queryKey: ["lark", "installations", "workspace-1"],
    });
    expect(toast.success).toHaveBeenCalledTimes(1);
  });

  it("toasts an error and keeps the badge mounted when the API call rejects", async () => {
    mockDeleteInstallation.mockRejectedValue(
      new Error("upstream 500"),
    );
    const user = userEvent.setup();
    renderUI(<LarkAgentBindButton agentId="agent-1" agentName="Bot" />);
    await user.click(screen.getByTestId("lark-agent-bot-disconnect"));
    const confirmButton = await screen.findByRole("button", {
      name: /^Disconnect$/i,
    });
    await user.click(confirmButton);

    await waitFor(() => {
      expect(toast.error).toHaveBeenCalledTimes(1);
    });
    // Cache must NOT be invalidated on failure — invalidating would
    // round-trip a refetch, momentarily flicker the row away even
    // though the bot is still installed server-side.
    expect(mockInvalidate).not.toHaveBeenCalled();
    // Badge stays mounted so the user can retry.
    expect(screen.getByTestId("lark-agent-bot-installed")).toBeTruthy();
  });

  it("disables the Cancel button while the request is in-flight (prevents racing the close)", async () => {
    let resolveDelete: () => void = () => {};
    mockDeleteInstallation.mockImplementation(
      () =>
        new Promise<void>((resolve) => {
          resolveDelete = resolve;
        }),
    );
    const user = userEvent.setup();
    renderUI(<LarkAgentBindButton agentId="agent-1" agentName="Bot" />);
    await user.click(screen.getByTestId("lark-agent-bot-disconnect"));
    const confirmButton = await screen.findByRole("button", {
      name: /^Disconnect$/i,
    });
    await user.click(confirmButton);

    // Cancel is disabled while disconnecting — closing mid-flight
    // would orphan the in-flight invalidate + toast.
    const cancel = screen.getByRole("button", { name: /Cancel/i });
    await waitFor(() => {
      expect((cancel as HTMLButtonElement).disabled).toBe(true);
    });

    // Resolve and let the request finish so jsdom doesn't carry a
    // dangling promise into the next test.
    resolveDelete();
    await waitFor(() => {
      expect(toast.success).toHaveBeenCalled();
    });
  });
});

describe("LarkInstallDialog (polling terminal errors)", () => {
  beforeEach(() => {
    resetFixtures();
    vi.useFakeTimers({ shouldAdvanceTime: true });
    mockBeginInstall.mockResolvedValue({
      session_id: "sess-1",
      qr_code_url: "https://accounts.feishu.cn/oauth/v1/device?u=abc",
      expires_in_seconds: 300,
      poll_interval_seconds: 2,
    });
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  async function openDialog() {
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    renderUI(<LarkAgentBindButton agentId="agent-1" agentName="Bot" />);
    // The Lark CTA is hidden (MUL-3083); open the dialog via the Feishu CTA
    // — the polling-error behavior under test is region-agnostic.
    await user.click(screen.getByRole("button", { name: /Bind to Feishu/i }));
    // Let the begin-session promise resolve and the QR render.
    await waitFor(() => {
      expect(screen.getByTestId("qr-code")).toBeTruthy();
    });
  }

  it("falls into a terminal session_lost error state when status polling 404s instead of looping forever", async () => {
    mockGetStatus.mockRejectedValue(
      new ApiError("install session not found", 404, "Not Found"),
    );

    await openDialog();
    // Drive the polling timer (intervalMs = max(2000, 2*1000)) and let
    // the rejected promise propagate into the catch.
    await act(async () => {
      await vi.advanceTimersByTimeAsync(2100);
    });

    await waitFor(() => {
      expect(
        screen.getByText(
          /Install session expired or was lost\. Scan again to start over\./i,
        ),
      ).toBeTruthy();
    });
    expect(screen.getByRole("button", { name: /Scan again/i })).toBeTruthy();
    // The dialog renders multiple Close affordances (footer button + the
    // built-in dialog dismiss); we only need to confirm at least one is
    // mounted alongside the retry button.
    expect(screen.getAllByRole("button", { name: /Close/i }).length).toBeGreaterThan(0);
  });

  it("treats 403 as a terminal forbidden error state (no infinite retry on revoked permission)", async () => {
    mockGetStatus.mockRejectedValue(
      new ApiError("forbidden", 403, "Forbidden"),
    );

    await openDialog();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(2100);
    });

    await waitFor(() => {
      expect(
        screen.getByText(
          /You no longer have permission to install Lark Bots in this workspace/i,
        ),
      ).toBeTruthy();
    });
    // Drive another full poll interval — the polling loop must NOT
    // schedule a follow-up fetch after a terminal 4xx.
    const callsAfterTerminal = mockGetStatus.mock.calls.length;
    await act(async () => {
      await vi.advanceTimersByTimeAsync(5000);
    });
    expect(mockGetStatus.mock.calls.length).toBe(callsAfterTerminal);
  });

  // Regression test for the empty-dialog bug Bohan hit on PR #3277:
  // the QR area was completely blank after opening the dialog. React 19
  // StrictMode dev mounts every component twice. The mount/cleanup/mount
  // cycle preserves the component's useRef across the simulated remount,
  // so the cleanup's `closedRef.current = true` survived into the
  // second mount. Both beginSession() promises then saw closedRef=true
  // at the post-await guard and skipped setSession(), leaving the dialog
  // body with no QR, no error, no loading text — just empty. Resetting
  // closedRef.current at the START of the effect re-arms the guard on
  // every mount.
  it("renders the QR after a React StrictMode double-mount (regression for empty dialog body)", async () => {
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    renderUI(
      <StrictMode>
        <LarkAgentBindButton agentId="agent-1" agentName="Bot" />
      </StrictMode>,
    );
    // The Lark CTA is hidden (MUL-3083); the StrictMode regression is about
    // the dialog mount cycle, so open it via the Feishu CTA.
    await user.click(screen.getByRole("button", { name: /Bind to Feishu/i }));

    // The QR must appear even though the dialog mounted, unmounted, and
    // mounted again under StrictMode. The previous bug left the body
    // empty here.
    await waitFor(
      () => {
        expect(screen.getByTestId("qr-code")).toBeTruthy();
      },
      { timeout: 2000 },
    );

    // And the QR's value should match what the (latest) begin call
    // returned — not be empty / undefined.
    const qr = screen.getByTestId("qr-code");
    expect(qr.getAttribute("data-value")).toBe(
      "https://accounts.feishu.cn/oauth/v1/device?u=abc",
    );
  });
});

// The installation list used to surface Lark's raw cli_… app_id and
// ou_… bot_open_id, which are meaningless to product users. The row now
// renders the Orvilo agent's avatar + name (joined via inst.agent_id),
// since the binding is 1:1 with an Agent. These tests pin that identity
// rendering so the row never regresses to leaking the cli_ prefix.
describe("LarkTab installation list (agent identity rendering)", () => {
  beforeEach(resetFixtures);

  it("renders the Orvilo agent's name and avatar instead of the raw Lark app_id / bot_open_id", async () => {
    agentNameByIdRef.current = new Map([["agent-1", "Bohan's Helper"]]);
    installationsRef.current.installations = [
      {
        id: "inst-1",
        workspace_id: "ws-1",
        agent_id: "agent-1",
        app_id: "cli_aa941499d4f95cd9",
        bot_open_id: "ou_abc123",
        installer_user_id: "user-1",
        status: "installed",
        installed_at: "2026-06-03T00:00:00Z",
        created_at: "2026-06-03T00:00:00Z",
        updated_at: "2026-06-03T00:00:00Z",
      },
    ];

    renderUI(<LarkTab />, { lobe: true });

    // The agent's display name is the primary identifier.
    expect(await screen.findByText("Bohan's Helper")).toBeTruthy();

    // The ActorAvatar stub records the actor it was asked to render —
    // confirms we joined on agent_id (and didn't accidentally pass the
    // bot_open_id or installation id).
    const avatar = screen.getByTestId("actor-avatar");
    expect(avatar.getAttribute("data-actor-type")).toBe("agent");
    expect(avatar.getAttribute("data-actor-id")).toBe("agent-1");

    // The raw Lark IDs are explicitly absent — the row must not leak
    // the cli_ / ou_ prefixes anymore.
    expect(screen.queryByText(/cli_aa941499d4f95cd9/)).toBeNull();
    expect(screen.queryByText(/ou_abc123/)).toBeNull();
  });

  it("falls back to a stable placeholder when the agent has been deleted (so the row is still actionable for cleanup)", async () => {
    // Empty map → useActorName.getAgentName returns "Unknown Agent".
    // The row must still render so admins can hit Disconnect.
    installationsRef.current.installations = [
      {
        id: "inst-orphan",
        workspace_id: "ws-1",
        agent_id: "agent-deleted",
        app_id: "cli_orphan",
        bot_open_id: "ou_orphan",
        installer_user_id: "user-1",
        status: "installed",
        installed_at: "2026-06-03T00:00:00Z",
        created_at: "2026-06-03T00:00:00Z",
        updated_at: "2026-06-03T00:00:00Z",
      },
    ];

    renderUI(<LarkTab />, { lobe: true });

    expect(await screen.findByText(/Unknown Agent/)).toBeTruthy();
    // Disconnect stays reachable so the orphan row can be cleaned up.
    expect(screen.getByRole("button", { name: /Disconnect/i })).toBeTruthy();
  });

  /**
   * The rethrow contract, asserted twice and at two layers, because the
   * DOM-level-only version of this test could not fail.
   *
   * `confirmModal` closes on the line after `onOk` unless `onOk` returns a
   * promise, and stays open when that promise rejects — so a disconnect that
   * fails must leave the confirmation up rather than dismiss as though the bot
   * were gone. The first test below keeps the DOM spelling where it can, but
   * waits a window in which the success path is shown to close; without that
   * demonstration "the title is still in the document" could be true because
   * nothing ever leaves it. The second drops the DOM entirely: the contract
   * lives in the promise `onOk` returns, and that is directly readable off the
   * config the wrapper handed the library.
   *
   * The subject-exists-in-both-states trap applies here in a second form: a row
   * assertion after a failed disconnect would be satisfied by the row, which is
   * rendered whether the request succeeded or not. Only the dialog's own
   * presence separates the two outcomes.
   */
  it("disconnects only after the confirmation, and the dialog closes when it succeeds", async () => {
    mockDeleteInstallation.mockResolvedValue(undefined);
    installationsRef.current.installations = [
      {
        id: "inst-1",
        workspace_id: "ws-1",
        agent_id: "agent-1",
        app_id: "cli_existing_app",
        bot_open_id: "ou_existing_bot",
        installer_user_id: "user-1",
        status: "installed",
        installed_at: "2026-06-03T00:00:00Z",
        created_at: "2026-06-03T00:00:00Z",
        updated_at: "2026-06-03T00:00:00Z",
      },
    ];

    renderUI(<LarkTab />, { lobe: true });
    await userEvent.click(await screen.findByRole("button", { name: /^Disconnect$/i }));
    expect(mockDeleteInstallation).not.toHaveBeenCalled();

    await screen.findByText("Disconnect this Lark bot?");
    // Two buttons carry this name — the row's and the dialog's — and the
    // dialog's is the last in document order (it is portalled to the end of
    // `<body>`), so `.at(-1)` is the one that runs the request.
    const confirmButtons = await screen.findAllByRole("button", { name: /^Disconnect$/i });
    await userEvent.click(confirmButtons.at(-1)!);
    await waitFor(() => {
      expect(mockDeleteInstallation).toHaveBeenCalledWith("workspace-1", "inst-1");
    });
    expect(mockInvalidate).toHaveBeenCalledWith({
      queryKey: ["lark", "installations", "workspace-1"],
    });
    expect(await dialogLeavesWithin(CLOSE_BUDGET_MS)).toBe(true);
  });

  it("keeps the confirmation up when the disconnect request fails", async () => {
    mockDeleteInstallation.mockRejectedValue(new Error("network failed"));
    installationsRef.current.installations = [
      {
        id: "inst-1",
        workspace_id: "ws-1",
        agent_id: "agent-1",
        app_id: "cli_existing_app",
        bot_open_id: "ou_existing_bot",
        installer_user_id: "user-1",
        status: "installed",
        installed_at: "2026-06-03T00:00:00Z",
        created_at: "2026-06-03T00:00:00Z",
        updated_at: "2026-06-03T00:00:00Z",
      },
    ];

    renderUI(<LarkTab />, { lobe: true });
    await userEvent.click(await screen.findByRole("button", { name: /^Disconnect$/i }));
    await screen.findByText("Disconnect this Lark bot?");
    const confirmButtons = await screen.findAllByRole("button", { name: /^Disconnect$/i });
    await userEvent.click(confirmButtons.at(-1)!);

    await waitFor(() => expect(toast.error).toHaveBeenCalledWith("network failed"));
    expect(await dialogLeavesWithin(CLOSE_WINDOW_MS)).toBe(false);
  });

  it("hands confirmModal an onOk that rejects when the disconnect fails", async () => {
    mockDeleteInstallation.mockRejectedValue(new Error("network failed"));
    installationsRef.current.installations = [
      {
        id: "inst-1",
        workspace_id: "ws-1",
        agent_id: "agent-1",
        app_id: "cli_existing_app",
        bot_open_id: "ou_existing_bot",
        installer_user_id: "user-1",
        status: "installed",
        installed_at: "2026-06-03T00:00:00Z",
        created_at: "2026-06-03T00:00:00Z",
        updated_at: "2026-06-03T00:00:00Z",
      },
    ];

    renderUI(<LarkTab />, { lobe: true });
    await userEvent.click(await screen.findByRole("button", { name: /^Disconnect$/i }));
    expect(capturedConfirm.current).not.toBeNull();

    await expect(capturedConfirm.current?.onOk()).rejects.toThrow("network failed");
    expect(toast.error).toHaveBeenCalledWith("network failed");
  });
});
