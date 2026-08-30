import { render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ReactNode } from "react";

const { search, sso } = vi.hoisted(() => ({
  search: { current: "" },
  sso: vi.fn(),
}));

vi.mock("@clerk/nextjs", () => ({
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
    window.history.replaceState(null, "", "/");
    search.current = "";
    sso.mockReset();
    sso.mockResolvedValue({ error: null });
    delete process.env.NEXT_PUBLIC_DESKTOP_APP_ORIGIN;
  });

  it("keeps the allowlisted browser app origin through the Google callback", async () => {
    const codeChallenge = "a".repeat(43);
    const state = "b".repeat(43);
    process.env.NEXT_PUBLIC_DESKTOP_APP_ORIGIN = "https://patchbay.aspectlylabs.com";
    search.current =
      `platform=desktop&code_challenge=${codeChallenge}&state=${state}` +
      "&app_origin=https%3A%2F%2Fpatchbay.aspectlylabs.com";

    render(<GoogleOAuthPage />);

    await waitFor(() => expect(sso).toHaveBeenCalledOnce());
    const query =
      `platform=desktop&code_challenge=${codeChallenge}&state=${state}` +
      "&app_origin=https%3A%2F%2Fpatchbay.aspectlylabs.com";
    expect(sso).toHaveBeenCalledWith(
      expect.objectContaining({
        redirectUrl: `/login?${query}`,
        redirectCallbackUrl: `/oauth/google/callback?${query}`,
      }),
    );
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

  it("fails closed before Clerk when the state binding is missing", async () => {
    search.current = `platform=desktop&code_challenge=${"a".repeat(43)}`;

    render(<GoogleOAuthPage />);

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Invalid desktop binding",
    );
    expect(sso).not.toHaveBeenCalled();
  });
});
