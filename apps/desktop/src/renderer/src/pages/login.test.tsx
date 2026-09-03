// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import "@testing-library/jest-dom/vitest";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ReactNode } from "react";
import { I18nProvider } from "@patchbay/core/i18n/react";
import { RESOURCES } from "@patchbay/views/locales";
import { DesktopLoginPage } from "./login";

const mocks = vi.hoisted(() => ({
  createGuestSession: vi.fn(),
  initiateDesktopAuthHandoff: vi.fn(),
  openExternal: vi.fn(),
  createDesktopGoogleLoginUrl: vi.fn(),
}));

vi.mock("@patchbay/core/auth", () => ({
  useAuthStore: (
    selector: (state: {
      createGuestSession: typeof mocks.createGuestSession;
    }) => unknown,
  ) => selector({ createGuestSession: mocks.createGuestSession }),
}));

vi.mock("@patchbay/core/api", () => ({
  api: { initiateDesktopAuthHandoff: mocks.initiateDesktopAuthHandoff },
}));

vi.mock("@patchbay/views/auth", () => ({
  LoginPage: ({
    logo,
    embedded,
    showGoogleSeparator,
    externalError,
    googleLoading,
    onGoogleLogin,
    extra,
  }: {
    logo?: ReactNode;
    embedded?: boolean;
    showGoogleSeparator?: boolean;
    externalError?: ReactNode;
    googleLoading?: boolean;
    onGoogleLogin?: () => void;
    extra?: ReactNode;
  }) => (
    <section
      data-testid="login-page"
      data-embedded={embedded ? "true" : "false"}
      data-show-google-separator={showGoogleSeparator ? "true" : "false"}
    >
      {logo}
      {externalError}
      {onGoogleLogin && (
        <button
          type="button"
          onClick={onGoogleLogin}
          disabled={googleLoading}
          aria-busy={googleLoading}
        >
          Continue with Google
        </button>
      )}
      {extra}
    </section>
  ),
}));

vi.mock("@patchbay/ui/components/common/patchbay-icon", () => ({
  PatchbayIcon: () => <div data-testid="patchbay-icon" />,
}));

vi.mock("@patchbay/views/platform", () => ({
  DragStrip: () => null,
}));

vi.mock("./login-handoff", () => ({
  createDesktopGoogleLoginUrl: mocks.createDesktopGoogleLoginUrl,
}));

function renderPage() {
  Object.defineProperty(window, "desktopAPI", {
    configurable: true,
    value: {
      runtimeConfig: {
        ok: true,
        config: { accountsUrl: "https://accounts.example" },
      },
      openExternal: mocks.openExternal,
    },
  });

  return render(
    <I18nProvider locale="en" resources={RESOURCES}>
      <DesktopLoginPage />
    </I18nProvider>,
  );
}

beforeEach(() => {
  vi.restoreAllMocks();
  mocks.createDesktopGoogleLoginUrl.mockImplementation(
    async (
      _accountsUrl: string,
      register: (state: string, challenge: string) => Promise<unknown>,
    ) => {
      await register("state-1", "challenge-1");
      return "https://accounts.example/google?state=state-1";
    },
  );
  mocks.initiateDesktopAuthHandoff.mockResolvedValue({ registered: true });
  mocks.openExternal.mockResolvedValue(undefined);
});

afterEach(() => {
  cleanup();
});

describe("DesktopLoginPage", () => {
  it("renders the brand panel beside the form panel", () => {
    renderPage();

    expect(
      screen.getByTestId("authentication-brand-panel"),
    ).toHaveTextContent("Patchbay");
    expect(
      screen.getByTestId("authentication-brand-panel"),
    ).toHaveTextContent("Sofia Davis");
    expect(screen.getByTestId("authentication-form-panel")).toContainElement(
      screen.getByTestId("login-page"),
    );
  });

  it("mounts the shared login page in embedded shadcn mode", () => {
    renderPage();

    expect(screen.getByTestId("login-page")).toHaveAttribute(
      "data-embedded",
      "true",
    );
    expect(screen.getByTestId("login-page")).toHaveAttribute(
      "data-show-google-separator",
      "true",
    );
  });

  it("continues as guest through the auth store", async () => {
    mocks.createGuestSession.mockResolvedValue({ is_guest: true });
    renderPage();

    fireEvent.click(screen.getByRole("button", { name: "Continue as guest" }));

    await waitFor(() => {
      expect(mocks.createGuestSession).toHaveBeenCalledOnce();
    });
    expect(
      screen.queryByText(/Could not start a guest session/),
    ).not.toBeInTheDocument();
  });

  it("shows a guest error when the server refuses the session", async () => {
    mocks.createGuestSession.mockRejectedValue(new Error("offline"));
    renderPage();

    fireEvent.click(screen.getByRole("button", { name: "Continue as guest" }));

    expect(
      await screen.findByText(/Could not start a guest session/),
    ).toBeInTheDocument();
  });

  it("opens Google login in the browser through the registered handoff", async () => {
    renderPage();

    fireEvent.click(
      screen.getByRole("button", { name: "Continue with Google" }),
    );

    await waitFor(() => {
      expect(mocks.initiateDesktopAuthHandoff).toHaveBeenCalledWith(
        "state-1",
        "challenge-1",
      );
    });
    expect(mocks.openExternal).toHaveBeenCalledWith(
      "https://accounts.example/google?state=state-1",
    );
  });

  it("disables Google while the browser handoff is opening", async () => {
    let release!: () => void;
    const gate = new Promise<unknown>((resolve) => {
      release = () => resolve({ registered: true });
    });
    mocks.initiateDesktopAuthHandoff.mockReturnValue(gate);
    renderPage();

    const button = screen.getByRole("button", {
      name: "Continue with Google",
    });
    fireEvent.click(button);

    expect(await screen.findByRole("button", { name: "Continue with Google" }))
      .toBeDisabled();
    release();
    await waitFor(() => {
      expect(
        screen.getByRole("button", { name: "Continue with Google" }),
      ).not.toBeDisabled();
    });
  });
});
