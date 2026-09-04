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
  SignIn: (props: Record<string, unknown>) => (
    <div
      data-testid="clerk-sign-in"
      data-routing={String(props.routing)}
      data-redirect={String(props.forceRedirectUrl)}
    />
  ),
  useAuth: () => ({ ...mocks.auth, getToken: mocks.getToken }),
  useClerk: () => ({ signOut: mocks.signOut }),
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
  it("registers the desktop attempt before rendering Clerk SignIn", async () => {
    render(<Page />);

    expect(screen.queryByTestId("clerk-sign-in")).not.toBeInTheDocument();
    await waitFor(() =>
      expect(mocks.register).toHaveBeenCalledWith({
        state: STATE,
        code_challenge: CHALLENGE,
      }),
    );
    expect(await screen.findByTestId("clerk-sign-in")).toHaveAttribute(
      "data-routing",
      "hash",
    );
    expect(screen.getByTestId("clerk-sign-in")).toHaveAttribute(
      "data-redirect",
      `/login?platform=desktop&state=${STATE}&code_challenge=${CHALLENGE}`,
    );
  });
});
