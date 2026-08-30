import { render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ReactNode } from "react";

const {
  clerkLoaded,
  session,
  search,
  signInResource,
  sso,
  signOut,
  replace,
  registerDesktopGoogleAttempt,
} =
  vi.hoisted(() => ({
    clerkLoaded: { current: true },
    session: { current: null as { id: string } | null },
    search: { current: "" },
    signInResource: { current: { sso: vi.fn() } as Record<string, unknown> },
    sso: vi.fn(),
    signOut: vi.fn(),
    replace: vi.fn(),
    registerDesktopGoogleAttempt: vi.fn(),
  }));

vi.mock("@patchbay/core/api", () => ({
  api: { registerDesktopGoogleAttempt },
}));

vi.mock("@clerk/nextjs", () => ({
  useClerk: () => ({
    loaded: clerkLoaded.current,
    session: session.current,
    signOut,
  }),
  useSignIn: () => ({ signIn: signInResource.current }),
}));

vi.mock("next/navigation", () => ({
  useRouter: () => ({ replace }),
  useSearchParams: () => new URLSearchParams(search.current),
}));

vi.mock("@/components/clerk-auth-shell", () => ({
  ClerkAuthShell: ({ children }: { children: ReactNode }) => children,
}));

vi.mock("@patchbay/views/i18n", () => ({
  useT: () => ({
    t: (
      select: (locale: {
        web: { google_oauth: Record<string, string> };
      }) => string,
    ) =>
      select({
        web: {
          google_oauth: {
            starting: "Opening Google sign-in…",
            invalid_binding: "Invalid desktop binding",
            failed: "Google sign-in failed",
          },
        },
      }),
  }),
}));

import GoogleOAuthPage from "./page";

describe("GoogleOAuthPage", () => {
  beforeEach(() => {
    vi.unstubAllGlobals();
    clerkLoaded.current = true;
    session.current = null;
    window.history.replaceState(null, "", "/");
    search.current = "";
    sso.mockReset();
    sso.mockResolvedValue({ error: null });
    signOut.mockReset();
    signOut.mockResolvedValue(undefined);
    replace.mockReset();
    registerDesktopGoogleAttempt.mockReset();
    registerDesktopGoogleAttempt.mockResolvedValue({ registered: true });
    signInResource.current = { sso };
  });

  it("starts a provider-specific Google sign-in with the renderer handoff intact", async () => {
    const codeChallenge = "a".repeat(43);
    const state = "b".repeat(43);
    search.current = `platform=desktop&code_challenge=${codeChallenge}&state=${state}`;

    render(<GoogleOAuthPage />);

    await waitFor(() => expect(sso).toHaveBeenCalledOnce());
    expect(registerDesktopGoogleAttempt).toHaveBeenCalledWith(
      state,
      codeChallenge,
    );
    const query = `platform=desktop&code_challenge=${codeChallenge}&state=${state}`;
    expect(sso).toHaveBeenCalledWith({
      strategy: "oauth_google",
      redirectUrl: `${window.location.origin}/login?${query}`,
      redirectCallbackUrl: `${window.location.origin}/oauth/google/callback?${query}`,
      oidcPrompt: "select_account",
    });
    expect(screen.queryByTestId("clerk-sign-in")).not.toBeInTheDocument();
  });

  it("preserves a configured broker base path through Clerk", async () => {
    const codeChallenge = "a".repeat(43);
    const state = "b".repeat(43);
    window.history.replaceState(null, "", "/patchbay/oauth/google");
    search.current = `platform=desktop&code_challenge=${codeChallenge}&state=${state}`;

    render(<GoogleOAuthPage />);

    await waitFor(() => expect(sso).toHaveBeenCalledOnce());
    const query = `platform=desktop&code_challenge=${codeChallenge}&state=${state}`;
    expect(sso).toHaveBeenCalledWith(
      expect.objectContaining({
        redirectUrl: `${window.location.origin}/patchbay/login?${query}`,
        redirectCallbackUrl: `${window.location.origin}/patchbay/oauth/google/callback?${query}`,
      }),
    );
  });

  it("fails closed before Clerk when the state binding is missing", async () => {
    search.current = `platform=desktop&code_challenge=${"a".repeat(43)}`;

    render(<GoogleOAuthPage />);

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Invalid desktop binding",
    );
    expect(sso).not.toHaveBeenCalled();
    expect(registerDesktopGoogleAttempt).not.toHaveBeenCalled();
  });

  it("does not leave for Google until Rust registers the desktop attempt", async () => {
    const codeChallenge = "a".repeat(43);
    const state = "b".repeat(43);
    search.current = `platform=desktop&code_challenge=${codeChallenge}&state=${state}`;
    registerDesktopGoogleAttempt.mockRejectedValue(new Error("unavailable"));

    render(<GoogleOAuthPage />);

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Google sign-in failed",
    );
    expect(sso).not.toHaveBeenCalled();
  });

  it("waits for Clerk readiness before consuming a Google attempt", async () => {
    const codeChallenge = "a".repeat(43);
    const state = "b".repeat(43);
    search.current = `platform=desktop&code_challenge=${codeChallenge}&state=${state}`;
    clerkLoaded.current = false;

    const view = render(<GoogleOAuthPage />);
    await Promise.resolve();
    expect(sso).not.toHaveBeenCalled();
    expect(registerDesktopGoogleAttempt).not.toHaveBeenCalled();

    clerkLoaded.current = true;
    view.rerender(<GoogleOAuthPage />);
    await waitFor(() => expect(sso).toHaveBeenCalledOnce());
  });

  it("waits until sso is actually available instead of failing closed", async () => {
    const codeChallenge = "a".repeat(43);
    const state = "b".repeat(43);
    search.current = `platform=desktop&code_challenge=${codeChallenge}&state=${state}`;
    signInResource.current = {};

    const view = render(<GoogleOAuthPage />);
    await Promise.resolve();
    expect(sso).not.toHaveBeenCalled();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();

    signInResource.current = { sso };
    view.rerender(<GoogleOAuthPage />);
    await waitFor(() => expect(sso).toHaveBeenCalledOnce());
  });

  it("clears only the active Clerk session before starting Google SSO", async () => {
    const codeChallenge = "a".repeat(43);
    const state = "b".repeat(43);
    search.current = `platform=desktop&code_challenge=${codeChallenge}&state=${state}`;
    session.current = { id: "sess_leftover" };
    window.history.replaceState(null, "", `/oauth/google?${search.current}`);
    signOut.mockImplementation(async () => {
      session.current = null;
    });

    const view = render(<GoogleOAuthPage />);

    await waitFor(() => expect(signOut).toHaveBeenCalledOnce());
    const resetQuery = `${search.current}&clerk_reset=1`;
    expect(signOut).toHaveBeenCalledWith({
      sessionId: "sess_leftover",
      redirectUrl: `${window.location.origin}/oauth/google?${resetQuery}`,
    });
    expect(sso).not.toHaveBeenCalled();
    expect(replace).not.toHaveBeenCalled();

    view.rerender(<GoogleOAuthPage />);
    await waitFor(() => expect(sso).toHaveBeenCalledOnce());
  });

  it("fails closed when Clerk cannot clear the ambient session", async () => {
    const codeChallenge = "a".repeat(43);
    const state = "b".repeat(43);
    search.current = `platform=desktop&code_challenge=${codeChallenge}&state=${state}`;
    session.current = { id: "sess_leftover" };
    signOut.mockRejectedValue(new Error("network failed"));

    render(<GoogleOAuthPage />);

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Google sign-in failed",
    );
    expect(sso).not.toHaveBeenCalled();
    expect(registerDesktopGoogleAttempt).toHaveBeenCalledWith(
      state,
      codeChallenge,
    );
  });

  it("does not loop when a reset returns with the session still active", async () => {
    const codeChallenge = "a".repeat(43);
    const state = "b".repeat(43);
    search.current = `platform=desktop&code_challenge=${codeChallenge}&state=${state}&clerk_reset=1`;
    session.current = { id: "sess_leftover" };

    render(<GoogleOAuthPage />);

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Google sign-in failed",
    );
    expect(signOut).not.toHaveBeenCalled();
    expect(sso).not.toHaveBeenCalled();
  });

  it("does not start a second SSO when Clerk already returned a ticket", async () => {
    const codeChallenge = "a".repeat(43);
    const state = "b".repeat(43);
    search.current = `platform=desktop&code_challenge=${codeChallenge}&state=${state}&rotating_token_nonce=nonce-value`;
    const locationReplace = vi.fn();
    vi.stubGlobal("location", {
      origin: "http://localhost:3000",
      pathname: "/oauth/google",
      search: `?${search.current}`,
      hash: "",
      href: `http://localhost:3000/oauth/google?${search.current}`,
      replace: locationReplace,
    });

    try {
      render(<GoogleOAuthPage />);

      await waitFor(() => expect(locationReplace).toHaveBeenCalledOnce());
      expect(locationReplace).toHaveBeenCalledWith(
        `/oauth/google/callback?${search.current}`,
      );
      expect(sso).not.toHaveBeenCalled();
      expect(registerDesktopGoogleAttempt).not.toHaveBeenCalled();
    } finally {
      vi.unstubAllGlobals();
    }
  });
});
