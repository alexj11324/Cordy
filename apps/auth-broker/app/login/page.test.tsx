// @vitest-environment jsdom

import { cleanup, render, screen, waitFor } from "@testing-library/react";
import "@testing-library/jest-dom/vitest";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import Page from "./page";

const STATE = "s".repeat(43);
const CHALLENGE = "c".repeat(43);

const mocks = vi.hoisted(() => ({
  register: vi.fn(),
  complete: vi.fn(),
  getToken: vi.fn(),
  signOut: vi.fn(),
  auth: { isLoaded: true, isSignedIn: false },
  searchParams: { current: new URLSearchParams() },
}));

vi.mock("next/navigation", () => ({
  useSearchParams: () => mocks.searchParams.current,
}));

vi.mock("@clerk/nextjs", () => ({
  useAuth: () => ({ ...mocks.auth, getToken: mocks.getToken }),
  useClerk: () => ({ signOut: mocks.signOut }),
}));

vi.mock("@/components/accounts-login-form", () => ({
  AccountsLoginForm: ({ returnUrl }: { returnUrl: string }) => (
    <div data-testid="accounts-login-form" data-return-url={returnUrl} />
  ),
}));

vi.mock("@/lib/broker-client", () => ({
  BrokerApiError: class BrokerApiError extends Error {
    constructor(public readonly status: number) {
      super(`Auth broker request failed (${status})`);
    }
  },
  registerDesktopGoogleAttempt: mocks.register,
  completeDesktopGoogleAttempt: mocks.complete,
}));

vi.mock("@/lib/auth-messages", () => ({
  useAuthMessages: () => ({
    brand: "Patchbay",
    quote: "Patchbay quote",
    login: "Login",
    desktopFailed: "The desktop sign-in could not be completed.",
    opening: "Opening Patchbay…",
  }),
}));

const originalFormSubmit = HTMLFormElement.prototype.submit;

beforeEach(() => {
  cleanup();
  HTMLFormElement.prototype.submit = originalFormSubmit;
  mocks.auth.isLoaded = true;
  mocks.auth.isSignedIn = false;
  mocks.register.mockReset().mockResolvedValue(undefined);
  mocks.complete.mockReset();
  mocks.getToken.mockReset();
  mocks.signOut.mockReset().mockResolvedValue(undefined);
  window.sessionStorage.clear();
  mocks.searchParams.current = new URLSearchParams({
    platform: "desktop",
    state: STATE,
    code_challenge: CHALLENGE,
  });
});

afterEach(() => {
  HTMLFormElement.prototype.submit = originalFormSubmit;
});

describe("Accounts desktop login", () => {
  it("registers the desktop attempt before rendering the custom login form", async () => {
    render(<Page />);

    expect(screen.queryByTestId("accounts-login-form")).not.toBeInTheDocument();
    await waitFor(() =>
      expect(mocks.register).toHaveBeenCalledWith({
        state: STATE,
        code_challenge: CHALLENGE,
      }),
    );
    expect(await screen.findByTestId("accounts-login-form")).toHaveAttribute(
      "data-return-url",
      `/login?platform=desktop&state=${STATE}&code_challenge=${CHALLENGE}`,
    );
  });

  it("renders the custom form for a direct Accounts login without a Desktop binding", async () => {
    mocks.searchParams.current = new URLSearchParams();

    render(<Page />);

    expect(await screen.findByTestId("accounts-login-form")).toHaveAttribute(
      "data-return-url",
      "https://patchbay.aspectlylabs.com/login",
    );
    expect(mocks.register).not.toHaveBeenCalled();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("accepts only the product return target for a direct Accounts login", async () => {
    mocks.searchParams.current = new URLSearchParams({
      return_url: "https://patchbay.aspectlylabs.com/acme/issues",
    });

    render(<Page />);

    expect(await screen.findByTestId("accounts-login-form")).toHaveAttribute(
      "data-return-url",
      "https://patchbay.aspectlylabs.com/acme/issues",
    );
  });

  it("does not register a production Google attempt for a loopback session API", async () => {
    mocks.searchParams.current = new URLSearchParams({
      platform: "desktop",
      state: STATE,
      code_challenge: CHALLENGE,
      session_api: "http://localhost:8080",
    });

    render(<Page />);

    expect(await screen.findByTestId("accounts-login-form")).toBeInTheDocument();
    expect(mocks.register).not.toHaveBeenCalled();
    expect(mocks.complete).not.toHaveBeenCalled();
  });

  it("does not post an ambient Clerk session to the loopback API", async () => {
    const submit = vi.fn();
    HTMLFormElement.prototype.submit = submit;
    mocks.auth.isSignedIn = true;
    mocks.getToken.mockResolvedValue("ambient-clerk-token");
    mocks.searchParams.current = new URLSearchParams({
      platform: "desktop",
      state: STATE,
      code_challenge: CHALLENGE,
      session_api: "http://localhost:8080",
    });

    render(<Page />);

    await waitFor(() => expect(mocks.signOut).toHaveBeenCalledOnce());
    expect(submit).not.toHaveBeenCalled();
    expect(mocks.complete).not.toHaveBeenCalled();
  });

  it("posts the Clerk session to the loopback product API after a fresh sign-in", async () => {
    const submit = vi.fn();
    HTMLFormElement.prototype.submit = submit;
    window.sessionStorage.setItem(
      `patchbay_desktop_loopback_fresh:${STATE}`,
      "1",
    );
    mocks.auth.isSignedIn = true;
    mocks.getToken.mockResolvedValue("clerk-session-token");
    mocks.searchParams.current = new URLSearchParams({
      platform: "desktop",
      state: STATE,
      code_challenge: CHALLENGE,
      session_api: "http://localhost:8080",
    });

    render(<Page />);

    await waitFor(() => expect(submit).toHaveBeenCalledOnce());
    const form = document.querySelector("form");
    expect(form?.getAttribute("action")).toBe(
      "http://localhost:8080/auth/desktop-session/complete",
    );
    expect(form?.getAttribute("method")).toBe("POST");
    expect(
      (form?.querySelector('input[name="session"]') as HTMLInputElement | null)
        ?.value,
    ).toBe("clerk-session-token");
    expect(mocks.complete).not.toHaveBeenCalled();
    expect(mocks.signOut).not.toHaveBeenCalled();
  });
});
