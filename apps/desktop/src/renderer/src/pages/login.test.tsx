// @vitest-environment jsdom

import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ReactNode } from "react";

const mocks = vi.hoisted(() => ({
  createGuestSession: vi.fn(),
  createGuestSessionForHandoff: vi.fn(),
  initiateDesktopGoogleAttempt: vi.fn(),
  logout: vi.fn(),
  openExternal: vi.fn(),
  setToken: vi.fn(),
}));

vi.mock("@patchbay/core/api", () => ({
  api: {
    initiateDesktopGoogleAttempt: mocks.initiateDesktopGoogleAttempt,
    logout: mocks.logout,
    setToken: mocks.setToken,
  },
}));

vi.mock("@patchbay/core/auth", () => ({
  useAuthStore: (
    selector: (state: {
      createGuestSession: typeof mocks.createGuestSession;
      createGuestSessionForHandoff: typeof mocks.createGuestSessionForHandoff;
      user: null;
    }) => unknown,
  ) =>
    selector({
      createGuestSession: mocks.createGuestSession,
      createGuestSessionForHandoff: mocks.createGuestSessionForHandoff,
      user: null,
    }),
}));

vi.mock("@patchbay/views/auth", () => ({
  LoginPage: ({
    embedded,
    showGoogleSeparator,
    googleLoading,
    onGoogleLogin,
    externalError,
    extra,
  }: {
    embedded?: boolean;
    showGoogleSeparator?: boolean;
    googleLoading?: boolean;
    onGoogleLogin?: () => void;
    externalError?: ReactNode;
    extra?: ReactNode;
  }) => (
    <section data-testid="email-otp-flow" data-embedded={embedded}>
      <div className="flex flex-col gap-2 text-center">
        <h1>Create an account</h1>
        <p>Enter your email below to create your account</p>
      </div>
      <div className="grid gap-6">
        {externalError}
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
  mocks.createGuestSessionForHandoff.mockReset();
  mocks.initiateDesktopGoogleAttempt.mockReset();
  mocks.logout.mockReset();
  mocks.openExternal.mockReset();
  mocks.setToken.mockReset();
  mocks.createGuestSession.mockResolvedValue({
    id: "guest-user",
    is_guest: true,
  });
  mocks.createGuestSessionForHandoff.mockResolvedValue({
    id: "guest-user",
    is_guest: true,
  });
  mocks.initiateDesktopGoogleAttempt.mockResolvedValue({ registered: true });
  mocks.logout.mockResolvedValue(undefined);
  Object.defineProperty(window, "desktopAPI", {
    configurable: true,
    value: {
      host: "electron",
      appInfo: {
        version: "0.2.4",
        os: "macos",
        authCallbackProtocol: "patchbay-canary-login-fix-123",
      },
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
  it("keeps the two-column authentication hierarchy within the available window", () => {
    render(<DesktopLoginPage />);

    const example = screen.getByTestId("authentication-example");
    expect(example).toHaveClass(
      "grid",
      "min-h-0",
      "w-full",
      "flex-1",
      "grid-cols-2",
    );
    expect(example).not.toHaveClass("container");
    expect(example).not.toHaveClass("shrink-0");
    expect(example).not.toHaveClass("overflow-hidden");
    const formPanel = screen.getByTestId("authentication-form-panel");
    expect(formPanel).toHaveClass("h-full", "min-h-0", "p-6", "lg:p-8");
    expect(formPanel.className).not.toMatch(/h-\[\d+px\]/);
    expect(screen.getByTestId("authentication-brand-panel")).toHaveClass(
      "flex",
      "h-full",
      "min-h-0",
      "p-10",
      "text-primary",
    );
    expect(screen.getByTestId("authentication-brand-panel")).not.toHaveClass(
      "hidden",
    );
    expect(screen.getByTestId("authentication-brand-panel")).not.toHaveClass(
      "min-h-[32rem]",
    );
    expect(screen.queryByRole("link", { name: "Login" })).toBeNull();
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
    expect(parsed.searchParams.get("callback_protocol")).toBeNull();
    expect(parsed.searchParams.get("code_challenge")).toHaveLength(43);
    expect(parsed.searchParams.get("state")).toHaveLength(43);
    expect(parsed.searchParams.get("token")).toBeNull();
    expect(mocks.initiateDesktopGoogleAttempt).toHaveBeenCalledWith(
      parsed.searchParams.get("state"),
      parsed.searchParams.get("code_challenge"),
      "patchbay-canary-login-fix-123",
    );
    expect(mocks.createGuestSessionForHandoff).toHaveBeenCalledOnce();
    expect(mocks.createGuestSession).not.toHaveBeenCalled();
  });

  it("exposes the real Email OTP form without opening the browser", () => {
    render(<DesktopLoginPage />);

    fireEvent.submit(screen.getByRole("form", { name: "Email sign in" }));

    expect(screen.getByLabelText("Email")).toBeInTheDocument();
    expect(mocks.openExternal).not.toHaveBeenCalled();
  });

  it("shows the Google loading state while the external login is opening", async () => {
    let finishOpen!: () => void;
    mocks.openExternal.mockReturnValue(
      new Promise<void>((resolve) => {
        finishOpen = resolve;
      }),
    );
    render(<DesktopLoginPage />);

    fireEvent.click(
      screen.getByRole("button", { name: "Continue with Google" }),
    );

    await waitFor(() => {
      expect(mocks.openExternal).toHaveBeenCalledOnce();
      expect(
        screen.getByRole("button", { name: "Opening Google sign-in…" }),
      ).toBeDisabled();
    });
    finishOpen();
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: "Continue with Google" }),
      ).toBeEnabled(),
    );
  });

  it("revokes the bootstrap guest when opening Google fails", async () => {
    mocks.openExternal.mockRejectedValueOnce(new Error("browser unavailable"));
    render(<DesktopLoginPage />);

    fireEvent.click(
      screen.getByRole("button", { name: "Continue with Google" }),
    );

    await waitFor(() =>
      expect(
        screen.getByText("Could not open the login page"),
      ).toBeInTheDocument(),
    );
    expect(mocks.logout).toHaveBeenCalledOnce();
    expect(mocks.setToken).toHaveBeenCalledWith(null);
  });

  it("starts the existing real guest session without opening the browser", async () => {
    render(<DesktopLoginPage />);

    fireEvent.click(screen.getByRole("button", { name: "Continue as guest" }));

    await waitFor(() =>
      expect(mocks.createGuestSession).toHaveBeenCalledOnce(),
    );
    expect(mocks.openExternal).not.toHaveBeenCalled();
  });
});
