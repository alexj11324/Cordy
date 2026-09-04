// @vitest-environment jsdom

import { render, screen, waitFor } from "@testing-library/react";
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
}));

vi.mock("next/navigation", () => ({
  useSearchParams: () =>
    new URLSearchParams({
      platform: "desktop",
      state: STATE,
      code_challenge: CHALLENGE,
    }),
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
  mocks.auth.isLoaded = true;
  mocks.auth.isSignedIn = false;
  mocks.register.mockReset().mockResolvedValue(undefined);
  mocks.complete.mockReset();
  mocks.getToken.mockReset();
  mocks.signOut.mockReset();
  window.sessionStorage.clear();
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
});
