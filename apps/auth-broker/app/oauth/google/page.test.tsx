// @vitest-environment jsdom

import { cleanup, render, screen, waitFor } from "@testing-library/react";
import "@testing-library/jest-dom/vitest";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import Page from "./page";

const mocks = vi.hoisted(() => ({
  searchParams: { current: new URLSearchParams() },
  sso: vi.fn(),
  register: vi.fn(),
  signIn: { sso: undefined as undefined | ReturnType<typeof vi.fn> },
  clerk: { loaded: true, session: null as null | { id: string } },
}));

vi.mock("next/navigation", () => ({
  useSearchParams: () => mocks.searchParams.current,
}));

vi.mock("@clerk/nextjs", () => ({
  useClerk: () => mocks.clerk,
  useSignIn: () => ({ signIn: mocks.signIn }),
}));

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
  mocks.signIn.sso = mocks.sso;
  mocks.clerk.loaded = true;
  mocks.clerk.session = null;
});

describe("Accounts Google entry", () => {
  it("starts when Clerk finishes loading without replacing its resource objects", async () => {
    mocks.clerk.loaded = false;
    const view = render(<Page />);
    expect(mocks.sso).not.toHaveBeenCalled();
    mocks.clerk.loaded = true;
    view.rerender(<Page />);
    await waitFor(() => expect(mocks.sso).toHaveBeenCalledOnce());
  });

  it("starts standalone Google OAuth without registering a Desktop attempt", async () => {
    render(<Page />);

    await waitFor(() => expect(mocks.sso).toHaveBeenCalledOnce());
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
    expect(mocks.sso.mock.calls[0]?.[0].redirectUrl).toContain("session_mode=local");
  });
});
