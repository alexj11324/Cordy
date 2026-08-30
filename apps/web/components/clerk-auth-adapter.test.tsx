import { act, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const { clerkState, getToken, loginWithClerk, logout, setAuthState, signOut } =
  vi.hoisted(() => ({
    clerkState: {
      current: {
        isLoaded: true,
        isSignedIn: true,
        sessionId: "session-a",
        userId: "user-a",
      },
    },
    getToken: vi.fn(),
    loginWithClerk: vi.fn(),
    logout: vi.fn(),
    setAuthState: vi.fn(),
    signOut: vi.fn(),
  }));

vi.mock("@clerk/nextjs", () => ({
  useUser: () => ({
    isLoaded: clerkState.current.isLoaded,
    user: clerkState.current.userId
      ? { id: clerkState.current.userId }
      : null,
  }),
  useAuth: () => ({
    getToken,
    isSignedIn: clerkState.current.isSignedIn,
    sessionId: clerkState.current.sessionId,
    signOut,
  }),
}));

vi.mock("@patchbay/core/auth", () => ({
  useAuthStore: Object.assign(vi.fn(), {
    getState: () => ({ loginWithClerk, logout }),
    setState: setAuthState,
  }),
}));

vi.mock("@patchbay/core/api", () => ({
  ApiError: class ApiError extends Error {
    status?: number;
  },
}));

import {
  ClerkAuthAdapter,
  useClerkSessionExchangeReady,
} from "./clerk-auth-adapter";

function ExchangeStatus() {
  return <output>{useClerkSessionExchangeReady() ? "ready" : "waiting"}</output>;
}

describe("ClerkAuthAdapter", () => {
  beforeEach(() => {
    clerkState.current = {
      isLoaded: true,
      isSignedIn: true,
      sessionId: "session-a",
      userId: "user-a",
    };
    getToken.mockReset().mockResolvedValue("clerk-session-token");
    loginWithClerk.mockReset().mockResolvedValue({ id: "patchbay-user" });
    logout.mockReset().mockResolvedValue(undefined);
    setAuthState.mockReset();
    signOut.mockReset().mockResolvedValue(undefined);
  });

  it("only marks the current Clerk identity ready after its Rust exchange", async () => {
    let finishSecondExchange: (() => void) | undefined;
    const secondExchange = new Promise<void>((resolve) => {
      finishSecondExchange = resolve;
    });
    loginWithClerk
      .mockResolvedValueOnce({ id: "patchbay-user-a" })
      .mockImplementationOnce(() => secondExchange);

    const { rerender } = render(
      <ClerkAuthAdapter>
        <ExchangeStatus />
      </ClerkAuthAdapter>,
    );

    await waitFor(() => expect(screen.getByText("ready")).toBeInTheDocument());

    clerkState.current = {
      isLoaded: true,
      isSignedIn: true,
      sessionId: "session-b",
      userId: "user-b",
    };
    rerender(
      <ClerkAuthAdapter>
        <ExchangeStatus />
      </ClerkAuthAdapter>,
    );

    await waitFor(() =>
      expect(screen.getByText("waiting")).toBeInTheDocument(),
    );
    expect(loginWithClerk).toHaveBeenCalledTimes(2);

    await act(async () => {
      finishSecondExchange?.();
      await secondExchange;
    });

    await waitFor(() => expect(screen.getByText("ready")).toBeInTheDocument());
  });
});
