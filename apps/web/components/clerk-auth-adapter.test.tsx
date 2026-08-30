import { act, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const {
  authState,
  clerkState,
  getToken,
  loginWithClerk,
  logout,
  setAuthState,
  signOut,
} = vi.hoisted(() => ({
  authState: {
    current: {
      status: "authenticating" as "authenticating" | "authenticated",
      retryGeneration: 0,
    },
  },
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
  useAuthStore: Object.assign(
    vi.fn((selector: (state: typeof authState.current) => unknown) =>
      selector(authState.current),
    ),
    {
      getState: () => ({ loginWithClerk, logout }),
      setState: setAuthState,
    },
  ),
}));

vi.mock("@patchbay/core/api", () => ({
  ApiError: class ApiError extends Error {
    status?: number;

    constructor(message: string, status: number, _statusText: string) {
      super(message);
      this.status = status;
    }
  },
}));

import {
  ClerkAuthAdapter,
  useClerkSessionExchangeReady,
} from "./clerk-auth-adapter";
import { ApiError } from "@patchbay/core/api";

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
    authState.current = { status: "authenticating", retryGeneration: 0 };
    getToken.mockReset().mockResolvedValue("clerk-session-token");
    loginWithClerk
      .mockReset()
      .mockImplementation(async () => {
        authState.current.status = "authenticated";
        return { id: "patchbay-user" };
      });
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
      .mockImplementationOnce(async () => {
        authState.current.status = "authenticated";
        return { id: "patchbay-user-a" };
      })
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

  it("re-exchanges the active Clerk identity after a failed local logout", async () => {
    const { rerender } = render(
      <ClerkAuthAdapter>
        <ExchangeStatus />
      </ClerkAuthAdapter>,
    );

    await waitFor(() => expect(screen.getByText("ready")).toBeInTheDocument());

    authState.current = { status: "authenticating", retryGeneration: 1 };
    rerender(
      <ClerkAuthAdapter>
        <ExchangeStatus />
      </ClerkAuthAdapter>,
    );

    await waitFor(() => expect(screen.getByText("waiting")).toBeInTheDocument());
    await waitFor(() => expect(loginWithClerk).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(screen.getByText("ready")).toBeInTheDocument());
  });

  it("suppresses auth re-arm while cleaning up a permanent exchange rejection", async () => {
    loginWithClerk.mockRejectedValueOnce(
      new ApiError("rejected", 401, "Unauthorized"),
    );

    render(
      <ClerkAuthAdapter>
        <ExchangeStatus />
      </ClerkAuthAdapter>,
    );

    await waitFor(() =>
      expect(logout).toHaveBeenCalledWith({ rearmAuth: false }),
    );
    expect(signOut).toHaveBeenCalledOnce();
  });
});
