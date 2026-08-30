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
  LoginPage: ({ extra }: { extra?: ReactNode }) => (
    <div data-testid="email-otp-flow">{extra}</div>
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
        common: { back: string };
        desktop: { entry: Record<string, string> };
      }) => string,
    ) =>
      select({
        common: { back: "Back" },
        desktop: {
          entry: {
            title: "Welcome to Patchbay",
            description: "Choose how to continue",
            login_google: "Continue with Google",
            opening_google: "Opening Google sign-in…",
            login_email: "Continue with Email",
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
  it("renders the branded equal Google and Email choices with Guest secondary", () => {
    render(<DesktopLoginPage />);

    expect(screen.getByTestId("patchbay-icon")).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Continue with Google" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Continue with Email" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Continue as guest" }),
    ).toBeInTheDocument();
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

  it("keeps the real Email OTP flow inside Electron", () => {
    render(<DesktopLoginPage />);

    fireEvent.click(
      screen.getByRole("button", { name: "Continue with Email" }),
    );

    expect(screen.getByTestId("email-otp-flow")).toBeInTheDocument();
    expect(mocks.openExternal).not.toHaveBeenCalled();
  });

  it("starts the existing real guest session without opening the browser", async () => {
    render(<DesktopLoginPage />);

    fireEvent.click(screen.getByRole("button", { name: "Continue as guest" }));

    await waitFor(() => expect(mocks.createGuestSession).toHaveBeenCalledOnce());
    expect(mocks.openExternal).not.toHaveBeenCalled();
  });
});
