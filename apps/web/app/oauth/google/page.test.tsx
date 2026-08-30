import { render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ReactNode } from "react";

const { authState, search, signOut, sso } = vi.hoisted(() => ({
  authState: { isLoaded: true, isSignedIn: false },
  search: { current: "" },
  signOut: vi.fn(),
  sso: vi.fn(),
}));

vi.mock("@clerk/nextjs", () => ({
  useAuth: () => ({
    isLoaded: authState.isLoaded,
    isSignedIn: authState.isSignedIn,
    signOut,
  }),
  useSignIn: () => ({ signIn: { sso } }),
}));

vi.mock("next/navigation", () => ({
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
    authState.isLoaded = true;
    authState.isSignedIn = false;
    window.history.replaceState(null, "", "/oauth/google");
    search.current = "";
    signOut.mockReset();
    signOut.mockResolvedValue(undefined);
    sso.mockReset();
    sso.mockResolvedValue({ error: null });
  });

  it("starts a provider-specific Google sign-in with the renderer handoff intact", async () => {
    const codeChallenge = "a".repeat(43);
    const state = "b".repeat(43);
    search.current = `platform=desktop&code_challenge=${codeChallenge}&state=${state}`;

    render(<GoogleOAuthPage />);

    await waitFor(() => expect(sso).toHaveBeenCalledOnce());
    const query = `platform=desktop&code_challenge=${codeChallenge}&state=${state}`;
    expect(sso).toHaveBeenCalledWith({
      strategy: "oauth_google",
      redirectUrl: `/login?${query}`,
      redirectCallbackUrl: `/oauth/google/callback?${query}`,
      oidcPrompt: "select_account",
    });
    expect(signOut).not.toHaveBeenCalled();
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
        redirectUrl: `/patchbay/login?${query}`,
        redirectCallbackUrl: `/patchbay/oauth/google/callback?${query}`,
      }),
    );
  });

  it("clears an existing Clerk session before starting the bound Google attempt", async () => {
    const codeChallenge = "a".repeat(43);
    const state = "b".repeat(43);
    const query = `platform=desktop&code_challenge=${codeChallenge}&state=${state}`;
    search.current = query;
    authState.isSignedIn = true;

    const view = render(<GoogleOAuthPage />);

    await waitFor(() => expect(signOut).toHaveBeenCalledOnce());
    expect(signOut).toHaveBeenCalledWith({
      redirectUrl: `/oauth/google?${query}`,
    });
    expect(sso).not.toHaveBeenCalled();

    // Clerk normally follows the redirect and remounts the page. Also support
    // clients that update auth state in place before navigation completes.
    authState.isSignedIn = false;
    view.rerender(<GoogleOAuthPage />);

    await waitFor(() => expect(sso).toHaveBeenCalledOnce());
    expect(sso).toHaveBeenCalledWith(
      expect.objectContaining({
        strategy: "oauth_google",
        oidcPrompt: "select_account",
      }),
    );
  });

  it("fails closed before Clerk when the state binding is missing", async () => {
    search.current = `platform=desktop&code_challenge=${"a".repeat(43)}`;

    render(<GoogleOAuthPage />);

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Invalid desktop binding",
    );
    expect(signOut).not.toHaveBeenCalled();
    expect(sso).not.toHaveBeenCalled();
  });

  it("waits for Clerk readiness before consuming a Google attempt", async () => {
    const codeChallenge = "a".repeat(43);
    const state = "b".repeat(43);
    search.current = `platform=desktop&code_challenge=${codeChallenge}&state=${state}`;
    authState.isLoaded = false;

    const view = render(<GoogleOAuthPage />);
    await Promise.resolve();
    expect(signOut).not.toHaveBeenCalled();
    expect(sso).not.toHaveBeenCalled();

    authState.isLoaded = true;
    view.rerender(<GoogleOAuthPage />);
    await waitFor(() => expect(sso).toHaveBeenCalledOnce());
  });
});
