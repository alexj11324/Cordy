import { render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ReactNode } from "react";

const { clerkLoaded, isSignedIn, search, signInResource, sso, replace } =
  vi.hoisted(() => ({
    clerkLoaded: { current: true },
    isSignedIn: { current: false },
    search: { current: "" },
    signInResource: { current: { sso: vi.fn() } as Record<string, unknown> },
    sso: vi.fn(),
    replace: vi.fn(),
  }));

vi.mock("@clerk/nextjs", () => ({
  useClerk: () => ({ loaded: clerkLoaded.current }),
  useAuth: () => ({ isSignedIn: isSignedIn.current }),
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
    isSignedIn.current = false;
    window.history.replaceState(null, "", "/");
    search.current = "";
    sso.mockReset();
    sso.mockResolvedValue({ error: null });
    replace.mockReset();
    signInResource.current = { sso };
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
  });

  it("waits for Clerk readiness before consuming a Google attempt", async () => {
    const codeChallenge = "a".repeat(43);
    const state = "b".repeat(43);
    search.current = `platform=desktop&code_challenge=${codeChallenge}&state=${state}`;
    clerkLoaded.current = false;

    const view = render(<GoogleOAuthPage />);
    await Promise.resolve();
    expect(sso).not.toHaveBeenCalled();

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

  it("hands an already-signed-in Clerk session to desktop login", async () => {
    const codeChallenge = "a".repeat(43);
    const state = "b".repeat(43);
    search.current = `platform=desktop&code_challenge=${codeChallenge}&state=${state}`;
    isSignedIn.current = true;

    render(<GoogleOAuthPage />);

    await waitFor(() =>
      expect(replace).toHaveBeenCalledWith(
        `/login?platform=desktop&code_challenge=${codeChallenge}&state=${state}`,
      ),
    );
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
    } finally {
      vi.unstubAllGlobals();
    }
  });
});
