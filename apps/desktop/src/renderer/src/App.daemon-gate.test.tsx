// @vitest-environment jsdom
import type { ReactNode } from "react";
import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const state = vi.hoisted(() => ({
  user: { id: "user-a" } as { id: string } | null,
  isLoading: false,
  status: "authenticated" as "authenticated" | "recovering",
  syncCalls: [] as string[],
  resolveUserB: undefined as (() => void) | undefined,
}));

const mocks = vi.hoisted(() => ({
  syncDaemonOnLogin: vi.fn(),
  setTargetApiUrl: vi.fn(async () => {}),
  onInviteOpen: vi.fn(() => () => {}),
}));

vi.mock("@patchbay/core/auth", () => ({
  useAuthStore: (
    selector: (value: typeof state) => unknown,
  ): unknown => selector(state),
}));

vi.mock("@tanstack/react-query", () => ({
  useQueryClient: () => ({ setQueryData: vi.fn() }),
}));

vi.mock("@patchbay/core/workspace", () => ({
  useWorkspaceList: () => ({
    workspaces: [{ id: "workspace-1", slug: "acme" }],
    ready: true,
    unavailable: false,
    isFetching: false,
    refetch: vi.fn(),
  }),
}));

vi.mock("@patchbay/core/workspace/queries", () => ({
  workspaceKeys: {
    list: () => ["workspace-list"],
    myInvitations: () => ["workspace-invitations"],
  },
}));

vi.mock("@patchbay/core/paths", () => ({
  useHasOnboarded: () => true,
}));

vi.mock("@patchbay/core/platform", () => ({
  setCurrentWorkspace: vi.fn(),
}));

vi.mock("@patchbay/core/api", () => ({
  api: {
    listMyInvitations: vi.fn(async () => []),
  },
}));

vi.mock("@patchbay/ui/components/common/patchbay-icon", () => ({
  PatchbayIcon: () => <div data-testid="daemon-loading" />,
}));

vi.mock("./pages/auth-recovery", () => ({
  DesktopAuthRecoveryPage: ({ errorReason }: { errorReason?: string }) => (
    <div data-testid="auth-recovery">{errorReason}</div>
  ),
}));

vi.mock("./pages/login", () => ({
  DesktopLoginPage: () => <div data-testid="desktop-login" />,
}));

vi.mock("./components/desktop-layout", () => ({
  DesktopShell: () => <div data-testid="desktop-shell" />,
}));

vi.mock("./stores/tab-store", () => ({
  useTabStore: Object.assign(
    (selector: (value: { activeWorkspaceSlug: string }) => unknown) =>
      selector({ activeWorkspaceSlug: "acme" }),
    {
      getState: () => ({
        validateWorkspaceSlugs: vi.fn(),
        switchWorkspace: vi.fn(),
      }),
    },
  ),
}));

vi.mock("./stores/window-overlay-store", () => ({
  useWindowOverlayStore: {
    getState: () => ({
      overlay: null,
      open: vi.fn(),
      close: vi.fn(),
      validateSettingsWorkspace: vi.fn(),
    }),
  },
}));

vi.mock("./platform/daemon-ipc-bridge", () => ({
  useDaemonIPCBridge: vi.fn(),
}));

vi.mock("./platform/daemon-login-sync", () => ({
  syncDaemonOnLogin: mocks.syncDaemonOnLogin,
}));

vi.mock("./platform/auth-session-bridge", () => ({
  DesktopAuthSessionBridge: ({ children }: { children?: ReactNode }) => (
    <>{children}</>
  ),
}));

// App.tsx imports these modules for the outer App component. Keep this gate
// test focused on AppContent without booting their runtime integrations.
vi.mock("./components/update-notification", () => ({
  UpdateNotification: () => null,
}));
vi.mock("./components/issue-window", () => ({
  IssueWindow: () => null,
}));
vi.mock("./platform/i18n-adapter", () => ({
  createDesktopLocaleAdapter: () => ({
    getUserChoice: () => null,
    getSystemPreferences: () => [],
    persist: vi.fn(),
  }),
}));
vi.mock("./pages/login-handoff", () => ({
  completeDesktopHandoff: vi.fn(),
}));
vi.mock("@patchbay/core/analytics", () => ({ captureEvent: vi.fn() }));
vi.mock("@patchbay/views/locales", () => ({ RESOURCES: {} }));
vi.mock("@patchbay/core/i18n", () => ({
  pickLocale: () => "en",
}));
vi.mock("@patchbay/core/onboarding", () => ({
  useWelcomeStore: { getState: () => ({ reset: vi.fn() }) },
}));
vi.mock("@patchbay/ui/components/common/theme-provider", () => ({
  ThemeProvider: ({ children }: { children?: ReactNode }) => <>{children}</>,
}));
vi.mock("@patchbay/ui/components/ui/sonner", () => ({ Toaster: () => null }));

const { AppContent } = await import("./App");

function installElectronGlobals() {
  Object.defineProperty(window, "desktopAPI", {
    configurable: true,
    value: {
      host: "electron",
      runtimeConfig: {
        ok: true,
        config: {
          apiUrl: "https://api.example.com",
          wsUrl: "wss://api.example.com/ws",
        },
      },
      setTargetApiUrl: mocks.setTargetApiUrl,
      onInviteOpen: mocks.onInviteOpen,
      onAuthHandoff: vi.fn(() => () => {}),
    },
  });
  Object.defineProperty(window, "daemonAPI", {
    configurable: true,
    value: {
      setTargetApiUrl: mocks.setTargetApiUrl,
      syncToken: vi.fn(),
      autoStart: vi.fn(),
      restart: vi.fn(),
    },
  });
  window.localStorage.setItem("patchbay_token", "renderer-session-token");
}

beforeEach(() => {
  state.user = { id: "user-a" };
  state.isLoading = false;
  state.status = "authenticated";
  state.syncCalls.length = 0;
  state.resolveUserB = undefined;
  mocks.syncDaemonOnLogin.mockReset();
  mocks.setTargetApiUrl.mockClear();
  mocks.onInviteOpen.mockClear();
  mocks.syncDaemonOnLogin.mockImplementation(
    (_api: unknown, _apiUrl: string, _token: string, userId: string) => {
      state.syncCalls.push(userId);
      if (userId === "user-a") return Promise.resolve();
      return new Promise<void>((resolve) => {
        state.resolveUserB = resolve;
      });
    },
  );
  installElectronGlobals();
});

afterEach(() => {
  cleanup();
  window.localStorage.clear();
});

describe("AppContent desktop daemon identity gate", () => {
  it("never mounts the shell with account A's ready state after switching to B", async () => {
    const view = render(<AppContent />);

    await waitFor(() => {
      expect(screen.getByTestId("desktop-shell")).toBeInTheDocument();
    });

    state.user = { id: "user-b" };
    view.rerender(<AppContent />);

    expect(screen.queryByTestId("desktop-shell")).toBeNull();
    expect(screen.getByTestId("daemon-loading")).toBeInTheDocument();
    expect(state.syncCalls).toEqual(["user-a", "user-b"]);

    await act(async () => {
      state.resolveUserB?.();
    });
    await waitFor(() => {
      expect(screen.getByTestId("desktop-shell")).toBeInTheDocument();
    });
  });

  it("blocks the shell and exposes the daemon failure for repair", async () => {
    const error = Object.assign(
      new Error("source-matched Patchbay CLI is unavailable; run pnpm dev"),
      { reason: "cli_not_found" },
    );
    mocks.syncDaemonOnLogin.mockRejectedValueOnce(error);

    render(<AppContent />);

    await waitFor(() => {
      expect(screen.getByTestId("auth-recovery")).toHaveTextContent(
        "cli_not_found",
      );
    });
    expect(screen.queryByTestId("desktop-shell")).toBeNull();
  });
});
