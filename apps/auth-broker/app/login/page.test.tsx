// @vitest-environment jsdom

import { cleanup, render, screen, waitFor } from "@testing-library/react";
import "@testing-library/jest-dom/vitest";
import { beforeEach, describe, expect, it, vi } from "vitest";
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

beforeEach(() => {
  cleanup();
  mocks.auth.isLoaded = true;
  mocks.auth.isSignedIn = false;
  mocks.register.mockReset().mockResolvedValue(undefined);
  mocks.complete.mockReset();
  mocks.getToken.mockReset();
  mocks.signOut.mockReset();
  window.sessionStorage.clear();
  mocks.searchParams.current = new URLSearchParams({
    platform: "desktop",
    state: STATE,
    code_challenge: CHALLENGE,
  });
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
});
