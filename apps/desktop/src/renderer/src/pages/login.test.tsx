import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ReactNode } from "react";

const mocks = vi.hoisted(() => ({
  createGuestSession: vi.fn(),
  openExternal: vi.fn(),
  isDesktopWebHost: vi.fn(() => false),
}));

vi.mock("@patchbay/core/auth", () => ({
  useAuthStore: (
    selector: (state: {
      createGuestSession: typeof mocks.createGuestSession;
    }) => unknown,
  ) => selector({ createGuestSession: mocks.createGuestSession }),
}));

vi.mock("@patchbay/views/auth", () => ({
  LoginPage: ({
    embedded,
    showGoogleSeparator,
    googleLoading,
    onGoogleLogin,
    extra,
  }: {
    embedded?: boolean;
    showGoogleSeparator?: boolean;
    googleLoading?: boolean;
    onGoogleLogin?: () => void;
    extra?: ReactNode;
  }) => (
    <section data-testid="email-otp-flow" data-embedded={embedded}>
      <div className="flex flex-col gap-2 text-center">
        <h1>Sign in to Patchbay</h1>
        <p>Enter your email to get a login code</p>
      </div>
      <div className="grid gap-6">
        <form aria-label="Email sign in">
          <label htmlFor="login-email">Email</label>
          <input id="login-email" type="email" />
          <button type="submit">Continue</button>
        </form>
        {showGoogleSeparator && (
          <div data-testid="google-separator">Or continue with</div>
        )}
        {onGoogleLogin && (
          <button
            type="button"
            onClick={onGoogleLogin}
            disabled={googleLoading}
            aria-busy={googleLoading}
          >
            {googleLoading ? "Opening Google sign-in…" : "Continue with Google"}
          </button>
        )}
      </div>
      {extra}
    </section>
  ),
}));

vi.mock("@patchbay/ui/components/common/patchbay-icon", () => ({
  PatchbayIcon: () => <div data-testid="patchbay-icon" />,
}));

vi.mock("@patchbay/views/onboarding", () => ({
  GoogleIcon: () => null,
}));

vi.mock("@patchbay/views/platform", () => ({
  DragStrip: () => null,
}));

vi.mock("../platform/web-bridge", () => ({
  isDesktopWebHost: mocks.isDesktopWebHost,
}));

vi.mock("@patchbay/views/i18n", () => ({
  useT: () => ({
    t: (
      select: (locale: {
        common: { or_continue_with: string };
        desktop: { entry: Record<string, string> };
      }) => string,
    ) =>
      select({
        common: { or_continue_with: "Or continue with" },
        desktop: {
          entry: {
            login_label: "Login",
            brand: "Patchbay",
            quote: "Patchbay keeps work in one clear place.",
            opening_google: "Opening Google sign-in…",
            skip: "Continue as guest",
            skipping: "Starting guest session…",
            login_error: "Could not open the login page",
            guest_error: "Could not start a guest session",
          },
        },
      }),
  }),
}));

import { DesktopLoginPage } from "./login";

beforeEach(() => {
  mocks.createGuestSession.mockReset();
  mocks.openExternal.mockReset();
  mocks.isDesktopWebHost.mockReset().mockReturnValue(false);
  mocks.createGuestSession.mockResolvedValue({
    id: "guest-user",
    is_guest: true,
  });
  Object.defineProperty(window, "desktopAPI", {
    configurable: true,
    value: {
      host: "electron",
      runtimeConfig: {
        ok: true,
        config: {
          appUrl: "https://patchbay.aspectlylabs.com",
          accountsUrl: "https://accounts.aspectlylabs.com",
        },
      },
      openExternal: mocks.openExternal,
    },
  });
});

describe("DesktopLoginPage", () => {
  it("keeps the authentication example hierarchy with Email first and Google second", () => {
    render(<DesktopLoginPage />);

    const example = screen.getByTestId("authentication-example");
    expect(example).toHaveClass("lg:grid-cols-2");
    expect(screen.getByTestId("authentication-brand-panel")).toHaveClass(
      "lg:flex",
    );
    expect(screen.getByRole("link", { name: "Login" })).toHaveAttribute(
      "href",
      "#desktop-login",
    );
    expect(screen.getByTestId("email-otp-flow")).toHaveAttribute(
      "data-embedded",
      "true",
    );
    expect(
      screen.getByRole("form", { name: "Email sign in" }),
    ).toBeInTheDocument();
    expect(screen.getByTestId("google-separator")).toHaveTextContent(
      "Or continue with",
    );
    expect(
      screen.getByRole("button", { name: "Continue with Google" }),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "Continue with Email" }),
    ).not.toBeInTheDocument();
  });

  it("opens the provider-specific Google route with the #623 PKCE binding", async () => {
    render(<DesktopLoginPage />);

    fireEvent.click(
      screen.getByRole("button", { name: "Continue with Google" }),
    );

    await waitFor(() => expect(mocks.openExternal).toHaveBeenCalledOnce());
    const [url] = mocks.openExternal.mock.calls[0] as [string];
    const parsed = new URL(url);
    expect(parsed.origin).toBe("https://accounts.aspectlylabs.com");
    expect(parsed.pathname).toBe("/oauth/google");
    expect(parsed.searchParams.get("platform")).toBe("desktop");
    expect(parsed.searchParams.get("code_challenge")).toHaveLength(43);
    expect(parsed.searchParams.get("state")).toHaveLength(43);
    expect(parsed.searchParams.get("token")).toBeNull();
  });

  it("exposes the real Email OTP form without opening the browser", () => {
    render(<DesktopLoginPage />);

    fireEvent.submit(screen.getByRole("form", { name: "Email sign in" }));

    expect(screen.getByLabelText("Email")).toBeInTheDocument();
    expect(mocks.openExternal).not.toHaveBeenCalled();
  });

  it("shows the Google loading state while the external login is opening", async () => {
    mocks.openExternal.mockReturnValue(new Promise(() => undefined));
    render(<DesktopLoginPage />);

    fireEvent.click(
      screen.getByRole("button", { name: "Continue with Google" }),
    );

    await waitFor(() => {
      expect(
        screen.getByRole("button", { name: "Opening Google sign-in…" }),
      ).toBeDisabled();
    });
  });

  it("starts the existing real guest session without opening the browser", async () => {
    render(<DesktopLoginPage />);

    fireEvent.click(screen.getByRole("button", { name: "Continue as guest" }));

    await waitFor(() => expect(mocks.createGuestSession).toHaveBeenCalledOnce());
    expect(mocks.openExternal).not.toHaveBeenCalled();
  });
});
