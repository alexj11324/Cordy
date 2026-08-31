import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const {
  signInProps,
  authState,
  search,
  authStoreState,
  issueCliToken,
  completeDesktopGoogleAttempt,
  redirectToCliCallback,
  redirectToDesktopApp,
  exchangeReady,
} = vi.hoisted(() => ({
  signInProps: { current: {} as Record<string, unknown> },
  authState: {
    current: { isLoaded: true, isSignedIn: false, getToken: vi.fn() },
  },
  search: { current: "" },
  authStoreState: { current: { status: "unauthenticated" } },
  issueCliToken: vi.fn(),
  completeDesktopGoogleAttempt: vi.fn(),
  redirectToCliCallback: vi.fn(),
  redirectToDesktopApp: vi.fn(),
  exchangeReady: { current: true },
}));

vi.mock("@patchbay/core/auth", () => ({
  useAuthStore: (selector: (state: { status: string }) => unknown) =>
    selector(authStoreState.current),
}));

vi.mock("@clerk/nextjs", () => ({
  SignIn: (props: Record<string, unknown>) => {
    signInProps.current = props;
    return <div data-testid="clerk-sign-in" />;
  },
  useAuth: () => authState.current,
}));

vi.mock("next/navigation", () => ({
  useSearchParams: () => new URLSearchParams(search.current),
}));

vi.mock("@patchbay/core/api", async (importOriginal) => {
  const original = await importOriginal<typeof import("@patchbay/core/api")>();
  return {
    ...original,
    api: { issueCliToken, completeDesktopGoogleAttempt },
  };
});

vi.mock("@patchbay/views/auth", async (importOriginal) => {
  const original =
    await importOriginal<typeof import("@patchbay/views/auth")>();
  return { ...original, redirectToCliCallback, redirectToDesktopApp };
});

vi.mock("@/components/clerk-auth-adapter", () => ({
  useClerkSessionExchangeReady: () => exchangeReady.current,
}));

vi.mock("@patchbay/views/i18n", () => ({
  useT: () => ({
    t: (
      selector: (resources: {
        web: {
          cli_authorization: Record<string, string>;
          desktop_handoff: Record<string, string>;
        };
      }) => string,
    ) =>
      selector({
        web: {
          cli_authorization: {
            prompt: "Localized CLI authorization prompt",
            authorize_button: "Localized CLI authorization button",
            failed: "Localized CLI authorization failure",
            invalid_callback: "Localized invalid CLI callback",
          },
          desktop_handoff: {
            opening_title: "Opening Patchbay",
            preparing: "Preparing Desktop sign-in...",
            opening_description: "Opening Patchbay Desktop",
            open_button: "Open Patchbay Desktop",
            prepare_failed: "Failed to prepare Desktop sign-in",
          },
        },
      }),
  }),
}));

import LoginPage from "./page";

describe("LoginPage", () => {
  beforeEach(() => {
    vi.unstubAllGlobals();
    signInProps.current = {};
    search.current = "";
    authStoreState.current = { status: "unauthenticated" };
    authState.current = {
      isLoaded: true,
      isSignedIn: false,
      getToken: vi.fn(),
    };
    exchangeReady.current = true;
    issueCliToken.mockReset();
    completeDesktopGoogleAttempt.mockReset();
    redirectToCliCallback.mockReset();
    redirectToDesktopApp.mockReset();
  });

  it("renders the Clerk sign-in flow at the canonical login route", () => {
    render(<LoginPage />);

    expect(screen.getByTestId("clerk-sign-in")).toBeInTheDocument();
    expect(signInProps.current).toMatchObject({
      routing: "path",
      path: "/login",
      signUpUrl: "/signup",
      forceRedirectUrl: "/",
    });
  });

  it("preserves a validated CLI callback through Clerk sign-in", () => {
    search.current =
      "cli_callback=http%3A%2F%2F127.0.0.1%3A43821%2Fcallback&cli_state=opaque-state";

    render(<LoginPage />);

    expect(signInProps.current.forceRedirectUrl).toBe(
      "/login?cli_callback=http%3A%2F%2F127.0.0.1%3A43821%2Fcallback&cli_state=opaque-state",
    );
  });

  it("localizes an invalid CLI callback", () => {
    search.current =
      "cli_callback=https%3A%2F%2Fevil.example%2Fcallback&cli_state=opaque-state";

    render(<LoginPage />);

    expect(screen.getByRole("alert")).toHaveTextContent(
      "Localized invalid CLI callback",
    );
    expect(screen.queryByTestId("clerk-sign-in")).not.toBeInTheDocument();
  });

  it("preserves the requested app path and query through Clerk sign-in", () => {
    search.current = "redirect_url=%2Fusage%3Ftab%3Dbilling%23summary";

    render(<LoginPage />);

    expect(signInProps.current.forceRedirectUrl).toBe(
      "/usage?tab=billing#summary",
    );
    expect(signInProps.current.signUpUrl).toBe(
      "/signup?redirect_url=%2Fusage%3Ftab%3Dbilling%23summary",
    );
  });

  it("rejects an external post-login redirect", () => {
    search.current = "redirect_url=https%3A%2F%2Fevil.example%2Ftakeover";

    render(<LoginPage />);

    expect(signInProps.current.forceRedirectUrl).toBe("/");
    expect(signInProps.current.signUpUrl).toBe("/signup");
  });

  it("preserves the desktop handoff through Clerk sign-in", () => {
    search.current =
      "platform=desktop&code_challenge=challenge-value&state=opaque-state";

    render(<LoginPage />);

    expect(signInProps.current).toMatchObject({
      signUpUrl:
        "/signup?platform=desktop&code_challenge=challenge-value&state=opaque-state",
      forceRedirectUrl:
        "/login?platform=desktop&code_challenge=challenge-value&state=opaque-state",
    });
  });

  it("offers CLI authorization after Clerk has established the session", () => {
    search.current =
      "cli_callback=http%3A%2F%2Flocalhost%3A43821%2Fcallback&cli_state=opaque-state";
    authState.current = { isLoaded: true, isSignedIn: true, getToken: vi.fn() };
    authStoreState.current = { status: "authenticated" };

    render(<LoginPage />);

    expect(
      screen.getByRole("button", {
        name: "Localized CLI authorization button",
      }),
    ).toBeInTheDocument();
    expect(
      screen.getByText("Localized CLI authorization prompt"),
    ).toBeInTheDocument();
    expect(screen.queryByTestId("clerk-sign-in")).not.toBeInTheDocument();
  });

  it("exchanges the Clerk session for a native Patchbay CLI token", async () => {
    search.current =
      "cli_callback=http%3A%2F%2Flocalhost%3A43821%2Fcallback&cli_state=opaque-state";
    authState.current = { isLoaded: true, isSignedIn: true, getToken: vi.fn() };
    authStoreState.current = { status: "authenticated" };
    issueCliToken.mockResolvedValue({ token: "patchbay-native-token" });

    render(<LoginPage />);
    fireEvent.click(
      screen.getByRole("button", {
        name: "Localized CLI authorization button",
      }),
    );

    await waitFor(() => expect(issueCliToken).toHaveBeenCalledOnce());
    expect(redirectToCliCallback).toHaveBeenCalledWith(
      "http://localhost:43821/callback",
      "patchbay-native-token",
      "opaque-state",
    );
    expect(authState.current.getToken).not.toHaveBeenCalled();
  });

  it("localizes a retryable CLI authorization failure", async () => {
    search.current =
      "cli_callback=http%3A%2F%2Flocalhost%3A43821%2Fcallback&cli_state=opaque-state";
    authState.current = { isLoaded: true, isSignedIn: true, getToken: vi.fn() };
    authStoreState.current = { status: "authenticated" };
    issueCliToken.mockRejectedValue(new Error("temporary failure"));

    render(<LoginPage />);
    fireEvent.click(
      screen.getByRole("button", {
        name: "Localized CLI authorization button",
      }),
    );

    await waitFor(() =>
      expect(screen.getByRole("alert")).toHaveTextContent(
        "Localized CLI authorization failure",
      ),
    );
    expect(redirectToCliCallback).not.toHaveBeenCalled();
  });

  it("does not authorize CLI before the Patchbay session exchange completes", () => {
    search.current =
      "cli_callback=http%3A%2F%2Flocalhost%3A43821%2Fcallback&cli_state=opaque-state";
    authState.current = { isLoaded: true, isSignedIn: true, getToken: vi.fn() };
    authStoreState.current = { status: "authenticating" };

    render(<LoginPage />);

    expect(screen.getByTestId("clerk-sign-in")).toBeInTheDocument();
    expect(
      screen.queryByRole("button", {
        name: "Localized CLI authorization button",
      }),
    ).not.toBeInTheDocument();
    expect(issueCliToken).not.toHaveBeenCalled();
  });

  it("automatically hands a signed-in desktop session to the Patchbay app", async () => {
    search.current =
      "platform=desktop&code_challenge=challenge-value&state=opaque-state&callback_protocol=patchbay-canary-attacker";
    authState.current = {
      isLoaded: true,
      isSignedIn: true,
      getToken: vi.fn().mockResolvedValue("clerk-session-token"),
    };
    authStoreState.current = { status: "authenticated" };
    completeDesktopGoogleAttempt.mockResolvedValue({
      callback_protocol: "patchbay-canary-login-fix-123",
      code: "desktop-handoff-code",
    });

    render(<LoginPage />);

    await waitFor(() =>
      expect(completeDesktopGoogleAttempt).toHaveBeenCalledOnce(),
    );
    expect(completeDesktopGoogleAttempt).toHaveBeenCalledWith(
      "clerk-session-token",
      "opaque-state",
      "challenge-value",
    );
    expect(redirectToDesktopApp).toHaveBeenCalledWith(
      "desktop-handoff-code",
      "opaque-state",
      "patchbay-canary-login-fix-123",
    );
  });

  it("waits for the current Clerk identity to finish the Rust session exchange", async () => {
    search.current =
      "platform=desktop&code_challenge=challenge-value&state=opaque-state";
    authState.current = { isLoaded: true, isSignedIn: true, getToken: vi.fn() };
    authStoreState.current = { status: "authenticated" };
    exchangeReady.current = false;

    render(<LoginPage />);

    expect(completeDesktopGoogleAttempt).not.toHaveBeenCalled();
    expect(
      screen.getByRole("button", { name: "Preparing Desktop sign-in..." }),
    ).toBeDisabled();
  });

  it("restarts Google instead of accepting an ambient signed-in session", async () => {
    const { ApiError } = await import("@patchbay/core/api");
    const codeChallenge = "c".repeat(43);
    const state = "s".repeat(43);
    search.current = `platform=desktop&code_challenge=${codeChallenge}&state=${state}`;
    authState.current = {
      isLoaded: true,
      isSignedIn: true,
      getToken: vi.fn().mockResolvedValue("ambient-clerk-token"),
    };
    authStoreState.current = { status: "authenticated" };
    completeDesktopGoogleAttempt.mockRejectedValue(
      new ApiError("fresh Google authorization is required", 409, "Conflict"),
    );
    const locationReplace = vi.fn();
    vi.stubGlobal("location", {
      pathname: "/login",
      replace: locationReplace,
    });

    render(<LoginPage />);

    await waitFor(() => expect(locationReplace).toHaveBeenCalledOnce());
    expect(locationReplace).toHaveBeenCalledWith(
      `/oauth/google?platform=desktop&code_challenge=${codeChallenge}&state=${state}`,
    );
    expect(redirectToDesktopApp).not.toHaveBeenCalled();
  });

  it("does not mint a desktop handoff without a renderer binding", async () => {
    search.current = "platform=desktop";
    authState.current = { isLoaded: true, isSignedIn: true, getToken: vi.fn() };
    authStoreState.current = { status: "authenticated" };

    render(<LoginPage />);

    await waitFor(() => expect(screen.getByRole("alert")).toBeInTheDocument());
    expect(completeDesktopGoogleAttempt).not.toHaveBeenCalled();
    expect(redirectToDesktopApp).not.toHaveBeenCalled();
  });
});
