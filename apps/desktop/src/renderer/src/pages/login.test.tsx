import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  createGuestSession: vi.fn(),
  sendCode: vi.fn(),
  verifyCode: vi.fn(),
  openExternal: vi.fn(),
}));

const authState = {
  createGuestSession: mocks.createGuestSession,
  sendCode: mocks.sendCode,
  verifyCode: mocks.verifyCode,
};

vi.mock("@patchbay/core/auth", () => ({
  useAuthStore: Object.assign(
    (selector: (state: typeof authState) => unknown) => selector(authState),
    { getState: () => authState },
  ),
}));

vi.mock("@patchbay/ui/components/common/patchbay-icon", () => ({
  PatchbayIcon: () => null,
}));

vi.mock("@patchbay/views/platform", () => ({
  DragStrip: () => null,
}));

vi.mock("@patchbay/views/i18n", () => ({
  useT: () => ({
    t: (select: (locale: Record<string, unknown>) => string) =>
      select({
        signin: {
          title: "Sign in to Patchbay",
          description: "Enter your email to get a login code",
          continue: "Continue",
          sending: "Sending code...",
          google: "Continue with Google",
        },
        verify: {
          title: "Check your email",
          description: "We sent a verification code to {{email}}",
          resend: "Resend code",
          resend_cooldown: "Resend in {{seconds}}s",
        },
        common: {
          back: "Back",
          email: "Email",
          email_placeholder: "you@example.com",
          email_required: "Email is required",
        },
        errors: {
          server_unreachable: "Make sure the server is running.",
          send_failed: "Failed to send code.",
          resend_failed: "Failed to resend code",
          code_invalid: "Invalid or expired code",
          cli_auth_failed: "Failed to authorize CLI. Please log in again.",
        },
        desktop: {
          entry: {
            title: "Sign in to Patchbay",
            description: "Use a real Patchbay workspace with or without an account.",
            login_google: "Continue with Google",
            opening_google: "Opening Google sign-in…",
            login_email: "Use email",
            opening_email: "Opening email sign-in…",
            skip: "Continue without signing in",
            skipping: "Starting guest session…",
            login_error: "Could not open the login page. Please try again.",
            guest_error: "Could not start a guest session",
          },
        },
      }),
  }),
}));

import { DesktopLoginPage } from "./login";

beforeEach(() => {
  mocks.createGuestSession.mockReset();
  mocks.sendCode.mockReset();
  mocks.verifyCode.mockReset();
  mocks.openExternal.mockReset();
  mocks.createGuestSession.mockResolvedValue({ id: "guest-user", is_guest: true });
  Object.defineProperty(window, "desktopAPI", {
    configurable: true,
    value: {
      runtimeConfig: {
        ok: true,
        config: { appUrl: "https://accounts.aspectlylabs.com" },
      },
      openExternal: mocks.openExternal,
    },
  });
});

function renderDesktopLogin() {
  return render(
    <QueryClientProvider client={new QueryClient()}>
      <DesktopLoginPage />
    </QueryClientProvider>,
  );
}

describe("DesktopLoginPage", () => {
  it("keeps formal login and offers a clear guest entry", () => {
    renderDesktopLogin();

    expect(
      screen.getByRole("button", { name: "Continue with Google" }),
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Use email" })).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Continue without signing in" }),
    ).toBeInTheDocument();
  });

  it("opens the configured public accounts login path for formal login", () => {
    renderDesktopLogin();

    fireEvent.click(screen.getByRole("button", { name: "Continue with Google" }));

    expect(mocks.openExternal).toHaveBeenCalledWith(
      "https://accounts.aspectlylabs.com/oauth/google?platform=desktop",
    );
    expect(mocks.createGuestSession).not.toHaveBeenCalled();
  });

  it("enters the in-app email and one-time-code flow", async () => {
    renderDesktopLogin();

    fireEvent.click(screen.getByRole("button", { name: "Use email" }));

    const email = screen.getByRole("textbox", { name: "Email" });
    expect(email).toBeInTheDocument();
    expect(mocks.openExternal).not.toHaveBeenCalled();

    mocks.sendCode.mockResolvedValueOnce(undefined);
    fireEvent.change(email, { target: { value: "person@example.com" } });
    fireEvent.click(screen.getByRole("button", { name: "Continue" }));

    await waitFor(() =>
      expect(mocks.sendCode).toHaveBeenCalledWith("person@example.com"),
    );
  });

  it("shows an actionable error when the system browser cannot open", async () => {
    mocks.openExternal.mockRejectedValueOnce(new Error("browser unavailable"));
    renderDesktopLogin();

    fireEvent.click(screen.getByRole("button", { name: "Continue with Google" }));

    await waitFor(() =>
      expect(
        screen.getByRole("alert", {
          name: "Could not open the login page. Please try again.",
        }),
      ).toBeInTheDocument(),
    );
  });

  it("starts a real guest session without opening the browser", async () => {
    renderDesktopLogin();

    fireEvent.click(screen.getByRole("button", { name: "Continue without signing in" }));

    await waitFor(() => expect(mocks.createGuestSession).toHaveBeenCalledOnce());
    expect(mocks.openExternal).not.toHaveBeenCalled();
  });
});
