import { describe, it, expect, vi, beforeEach } from "vitest";
import { act, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { configStore } from "@patchbay/core/config";
import { I18nProvider } from "@patchbay/core/i18n/react";
import enCommon from "@patchbay/views/locales/en/common.json";
import enAuth from "@patchbay/views/locales/en/auth.json";
import enSettings from "@patchbay/views/locales/en/settings.json";
import type { ReactNode } from "react";

const TEST_RESOURCES = {
  en: { common: enCommon, auth: enAuth, settings: enSettings },
};

function createWrapper() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return ({ children }: { children: ReactNode }) => (
    <I18nProvider locale="en" resources={TEST_RESOURCES}>
      <QueryClientProvider client={qc}>{children}</QueryClientProvider>
    </I18nProvider>
  );
}

const {
  mockCompleteDesktopAuthHandoff,
  mockListWorkspaces,
  mockListMyInvitations,
  mockPush,
  mockReplace,
  searchParamsState,
  authStateRef,
} = vi.hoisted(() => ({
  mockCompleteDesktopAuthHandoff: vi.fn(),
  mockListWorkspaces: vi.fn(),
  mockListMyInvitations: vi.fn(),
  mockPush: vi.fn(),
  mockReplace: vi.fn(),
  searchParamsState: { params: new URLSearchParams() },
  authStateRef: {
    state: {
      sendCode: vi.fn(),
      verifyCode: vi.fn(),
      user: null as null | { id: string; email: string; onboarded_at?: string | null },
      isLoading: false,
    },
  },
}));

// Mock next/navigation — router spies are hoisted so tests can assert
// which navigation (if any) the page issued.
vi.mock("next/navigation", () => ({
  useRouter: () => ({ push: mockPush, replace: mockReplace }),
  usePathname: () => "/login",
  useSearchParams: () => searchParamsState.params,
}));

// Mock auth store — shared LoginPage uses getState().sendCode/verifyCode,
// web wrapper uses useAuthStore((s) => s.user/isLoading). Keep the real
// sanitizeNextUrl so the redirect-sanitization rules are exercised rather
// than silently drifting behind a mock reimplementation.
vi.mock("@patchbay/core/auth", async () => {
  const actual =
    await vi.importActual<typeof import("@patchbay/core/auth")>(
      "@patchbay/core/auth",
    );
  const useAuthStore = Object.assign(
    (selector: (s: typeof authStateRef.state) => unknown) =>
      selector(authStateRef.state),
    { getState: () => authStateRef.state },
  );
  return { ...actual, useAuthStore };
});

// Mock auth-cookie
vi.mock("@/features/auth/auth-cookie", () => ({
  setLoggedInCookie: vi.fn(),
}));

// Mock api
vi.mock("@patchbay/core/api", () => ({
  api: {
    listWorkspaces: mockListWorkspaces,
    listMyInvitations: mockListMyInvitations,
    verifyCode: vi.fn(),
    setToken: vi.fn(),
    getMe: vi.fn(),
    completeDesktopAuthHandoff: mockCompleteDesktopAuthHandoff,
  },
}));

import LoginPage from "./page";

describe("LoginPage", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    searchParamsState.params = new URLSearchParams();
    authStateRef.state.user = null;
    authStateRef.state.isLoading = false;
    configStore.getState().setAuthConfig({
      allowSignup: true,
      googleClientId: "google-client-id",
    });
    mockListWorkspaces.mockResolvedValue([]);
    mockListMyInvitations.mockResolvedValue([]);
  });

  // Shared LoginPage behavior is canonical in
  // packages/views/auth/login-page.test.tsx. This wrapper suite only owns web
  // platform handoff and redirect behavior.

  it("renders the approved connected-dot Patchbay mark", () => {
    const { container } = render(<LoginPage />, { wrapper: createWrapper() });

    expect(
      container.querySelector('svg[viewBox="0 0 128 128"]'),
    ).toBeInTheDocument();
  });

  it("keeps ordinary Web login on email send-code and hides the Google broker", () => {
    render(<LoginPage />, { wrapper: createWrapper() });

    expect(
      screen.queryByRole("button", { name: /continue with google/i }),
    ).not.toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: /^Continue$/ }),
    ).toBeInTheDocument();
  });

  it("keeps Google available for an explicit Desktop handoff", () => {
    searchParamsState.params = new URLSearchParams({
      platform: "desktop",
      state: "state-a",
      code_challenge: "challenge-a",
    });

    render(<LoginPage />, { wrapper: createWrapper() });

    expect(
      screen.getByRole("button", { name: /continue with google/i }),
    ).toBeInTheDocument();
  });

  // Regression: the browser must complete the registered PKCE binding and
  // hand only a one-time code to Desktop, never a bearer JWT in the URI.
  it("completes a PKCE handoff and deep-links to Desktop when already logged in", async () => {
    searchParamsState.params = new URLSearchParams({
      platform: "desktop",
      state: "state-a",
      code_challenge: "challenge-a",
    });
    authStateRef.state.user = { id: "u1", email: "test@patchbay.ai" };
    mockCompleteDesktopAuthHandoff.mockImplementation(() =>
      Promise.resolve({
        callback_protocol: "patchbay",
        code: "handoff-code",
        state: "state-a",
      }),
    );

    const hrefSetter = vi.fn();
    const originalLocation = window.location;
    Object.defineProperty(window, "location", {
      configurable: true,
      value: { ...originalLocation, set href(value: string) { hrefSetter(value); } },
    });

    try {
      render(<LoginPage />, { wrapper: createWrapper() });

      await waitFor(() => {
        expect(mockCompleteDesktopAuthHandoff).toHaveBeenCalledWith(
          "state-a",
          "challenge-a",
        );
      });
      await waitFor(() => {
        expect(hrefSetter).toHaveBeenCalledWith(
          "patchbay://auth/callback?code=handoff-code&state=state-a",
        );
      });
      expect(screen.getByText("Opening Patchbay")).toBeInTheDocument();
    } finally {
      Object.defineProperty(window, "location", {
        configurable: true,
        value: originalLocation,
      });
    }
  });

  // Regression: #5009 — the "already authenticated on arrival" effect used to
  // fire for fresh form logins too. verifyCode writes `user` while handleVerify
  // is still fetching the workspace list, so the effect read an empty cache and
  // raced handleSuccess with replace("/workspaces/new"); depending on the
  // interleaving the user could end up stuck on the create-workspace page
  // despite having workspaces.
  describe("post-login redirect ownership (#5009)", () => {
    const onboardedUser = {
      id: "u1",
      email: "test@patchbay.ai",
      onboarded_at: "2026-01-01T00:00:00Z",
    };

    it("does not redirect from the arrival effect when the user logs in via the form", async () => {
      // Auth settles as logged-out first — the page latches "any user from
      // now on came from the form".
      const wrapper = createWrapper();
      const { rerender } = render(<LoginPage />, { wrapper });
      // verifyCode set the user; the workspace list fetch is still in flight
      // (cache cold). The arrival effect must stay silent — handleSuccess
      // owns this navigation.
      authStateRef.state.user = onboardedUser;
      rerender(<LoginPage />);

      await act(async () => {});
      expect(mockReplace).not.toHaveBeenCalled();
      expect(mockPush).not.toHaveBeenCalled();
      expect(mockListWorkspaces).not.toHaveBeenCalled();
    });

    it("fetches the workspace list before redirecting a visitor who arrived authenticated", async () => {
      // Cold Query cache on a fresh page load: reading it would say "no
      // workspaces" and misroute to /workspaces/new. The effect must fetch.
      authStateRef.state.user = onboardedUser;
      mockListWorkspaces.mockResolvedValue([{ id: "ws-1", slug: "acme" }]);

      render(<LoginPage />, { wrapper: createWrapper() });

      await waitFor(() => {
        expect(mockReplace).toHaveBeenCalledWith("/acme/issues");
      });
      expect(mockListWorkspaces).toHaveBeenCalledTimes(1);
    });

    it("still honors ?next= for a visitor who arrived authenticated", async () => {
      searchParamsState.params = new URLSearchParams({
        next: "/invite/abc",
      });
      authStateRef.state.user = onboardedUser;

      render(<LoginPage />, { wrapper: createWrapper() });

      await waitFor(() => {
        expect(mockReplace).toHaveBeenCalledWith("/invite/abc");
      });
      expect(mockListWorkspaces).not.toHaveBeenCalled();
    });
  });
});
