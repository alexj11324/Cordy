import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const {
  signInProps,
  authState,
  search,
  authStoreState,
  issueCliToken,
  redirectToCliCallback,
  redirectToDesktopApp,
} = vi.hoisted(() => ({
  signInProps: { current: {} as Record<string, unknown> },
  authState: {
    current: { isLoaded: true, isSignedIn: false, getToken: vi.fn() },
  },
  search: { current: "" },
  authStoreState: { current: { status: "unauthenticated" } },
  issueCliToken: vi.fn(),
  redirectToCliCallback: vi.fn(),
  redirectToDesktopApp: vi.fn(),
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

vi.mock("@patchbay/core/api", () => ({
  api: { issueCliToken },
}));

vi.mock("@patchbay/views/auth", async (importOriginal) => {
  const original = await importOriginal<typeof import("@patchbay/views/auth")>();
  return { ...original, redirectToCliCallback, redirectToDesktopApp };
});

vi.mock("@patchbay/views/i18n", () => ({
  useT: () => ({ t: () => "Open Patchbay Desktop" }),
}));

import LoginPage from "./page";

describe("LoginPage", () => {
  beforeEach(() => {
    signInProps.current = {};
    search.current = "";
    authStoreState.current = { status: "unauthenticated" };
    authState.current = { isLoaded: true, isSignedIn: false, getToken: vi.fn() };
    issueCliToken.mockReset();
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

  it("preserves the requested app path and query through Clerk sign-in", () => {
    search.current = "redirect_url=%2Fusage%3Ftab%3Dbilling%23summary";

    render(<LoginPage />);

    expect(signInProps.current.forceRedirectUrl).toBe(
      "/usage?tab=billing#summary",
    );
  });

  it("rejects an external post-login redirect", () => {
    search.current =
      "redirect_url=https%3A%2F%2Fevil.example%2Ftakeover";

    render(<LoginPage />);

    expect(signInProps.current.forceRedirectUrl).toBe("/");
  });

  it("preserves the desktop handoff through Clerk sign-in", () => {
    search.current = "platform=desktop";

    render(<LoginPage />);

    expect(signInProps.current).toMatchObject({
      signUpUrl: "/signup?platform=desktop",
      forceRedirectUrl: "/login?platform=desktop",
    });
  });

  it("offers CLI authorization after Clerk has established the session", () => {
    search.current =
      "cli_callback=http%3A%2F%2Flocalhost%3A43821%2Fcallback&cli_state=opaque-state";
    authState.current = { isLoaded: true, isSignedIn: true, getToken: vi.fn() };

    render(<LoginPage />);

    expect(
      screen.getByRole("button", { name: "Authorize CLI" }),
    ).toBeInTheDocument();
    expect(screen.queryByTestId("clerk-sign-in")).not.toBeInTheDocument();
  });

  it("exchanges the Clerk session for a native Patchbay CLI token", async () => {
    search.current =
      "cli_callback=http%3A%2F%2Flocalhost%3A43821%2Fcallback&cli_state=opaque-state";
    authState.current = { isLoaded: true, isSignedIn: true, getToken: vi.fn() };
    issueCliToken.mockResolvedValue({ token: "patchbay-native-token" });

    render(<LoginPage />);
    fireEvent.click(screen.getByRole("button", { name: "Authorize CLI" }));

    await waitFor(() => expect(issueCliToken).toHaveBeenCalledOnce());
    expect(redirectToCliCallback).toHaveBeenCalledWith(
      "http://localhost:43821/callback",
      "patchbay-native-token",
      "opaque-state",
    );
    expect(authState.current.getToken).not.toHaveBeenCalled();
  });

  it("automatically hands a signed-in desktop session to the Patchbay app", async () => {
    search.current = "platform=desktop";
    authState.current = { isLoaded: true, isSignedIn: true, getToken: vi.fn() };
    authStoreState.current = { status: "authenticated" };
    issueCliToken.mockResolvedValue({ token: "desktop-native-token" });

    render(<LoginPage />);

    await waitFor(() => expect(issueCliToken).toHaveBeenCalledOnce());
    expect(redirectToDesktopApp).toHaveBeenCalledWith("desktop-native-token");
  });
});
