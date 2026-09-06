// @vitest-environment jsdom

import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import "@testing-library/jest-dom/vitest";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import Page from "./page";

const mocks = vi.hoisted(() => {
  const client = { signIn: { __internal_future: { sso: vi.fn() } }, resetSignIn: vi.fn() };
  return {
    searchParams: { current: new URLSearchParams() },
    sso: vi.fn(),
    register: vi.fn(),
    listeners: new Set<() => void>(),
    client,
    clerk: { loaded: true, session: null as null | { id: string }, client },
  };
});

vi.mock("next/navigation", () => ({
  useSearchParams: () => mocks.searchParams.current,
}));

vi.mock("@clerk/nextjs", async () => {
  const { useSyncExternalStore } = await import("react");
  return {
  useAuth: () => ({ isLoaded: useSyncExternalStore(
    (listener) => { mocks.listeners.add(listener); return () => { mocks.listeners.delete(listener); }; },
    () => mocks.clerk.loaded,
  ) }),
  useClerk: () => mocks.clerk,
  };
});

vi.mock("@/components/auth-shell", () => ({
  AuthShell: ({ children }: { children: React.ReactNode }) => (
    <main>{children}</main>
  ),
}));

vi.mock("@/lib/broker-client", () => ({
  registerDesktopGoogleAttempt: mocks.register,
}));

vi.mock("@/lib/auth-messages", () => ({
  useAuthMessages: () => ({
    starting: "Starting",
    startFailed: "Start failed",
    retry: "Retry",
  }),
}));

afterEach(cleanup);

beforeEach(() => {
  window.sessionStorage.clear();
  mocks.searchParams.current = new URLSearchParams({
    return_url: "https://patchbay.aspectlylabs.com/login",
  });
  mocks.sso.mockReset().mockResolvedValue({ error: null });
  mocks.register.mockReset().mockResolvedValue(undefined);
  mocks.client.resetSignIn.mockReset().mockImplementation(() => {
    mocks.client.signIn = { __internal_future: { sso: mocks.sso } };
  });
  mocks.client.signIn = { __internal_future: { sso: vi.fn() } };
  mocks.clerk.loaded = true;
  mocks.clerk.session = null;
});

describe("Accounts Google entry", () => {
  it("starts when Clerk finishes loading without replacing its resource objects", async () => {
    mocks.clerk.loaded = false;
    render(<Page />);
    expect(mocks.sso).not.toHaveBeenCalled();
    act(() => {
      mocks.clerk.loaded = true;
      mocks.listeners.forEach((listener) => listener());
    });
    await waitFor(() => expect(mocks.sso).toHaveBeenCalledOnce());
  });

  it("starts standalone Google OAuth without registering a Desktop attempt", async () => {
    const staleSso = mocks.client.signIn.__internal_future.sso;
    render(<Page />);

    await waitFor(() => expect(mocks.sso).toHaveBeenCalledOnce());
    expect(staleSso).not.toHaveBeenCalled();
    expect(mocks.client.resetSignIn).toHaveBeenCalledOnce();
    const call = mocks.sso.mock.calls[0]?.[0] as {
      redirectUrl: string;
      redirectCallbackUrl: string;
    };
    expect(new URL(call.redirectUrl).searchParams.get("return_url")).toBe(
      "https://patchbay.aspectlylabs.com/login",
    );
    expect(
      new URL(call.redirectCallbackUrl).searchParams.get("return_url"),
    ).toBe("https://patchbay.aspectlylabs.com/login");
    expect(screen.getByRole("status")).toHaveTextContent("Starting");
  });

  it("registers the local identity attempt with the hosted broker before Google", async () => {
    const state = "s".repeat(43);
    const challenge = "c".repeat(43);
    mocks.searchParams.current = new URLSearchParams({
      platform: "desktop",
      state,
      code_challenge: challenge,
      session_mode: "local",
    });

    render(<Page />);

    await waitFor(() => expect(mocks.sso).toHaveBeenCalledOnce());
    expect(mocks.register).toHaveBeenCalledWith({ state, code_challenge: challenge });
    expect(mocks.client.resetSignIn).toHaveBeenCalledOnce();
    expect(mocks.client.resetSignIn.mock.invocationCallOrder[0]).toBeGreaterThan(mocks.register.mock.invocationCallOrder[0]!);
    expect(mocks.sso.mock.calls[0]?.[0].redirectUrl).toContain("session_mode=local");
  });
});
