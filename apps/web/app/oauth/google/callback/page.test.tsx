import { render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ReactNode } from "react";

type Navigate = (input: {
  session: { currentTask: null };
  decorateUrl: (url: string) => string;
}) => Promise<void>;

const mocks = vi.hoisted(() => ({
  search: { current: "" },
  replace: vi.fn(),
  signIn: {
    status: "complete",
    isTransferable: false,
    existingSession: null as { sessionId: string } | null,
    create: vi.fn(),
    finalize: vi.fn(),
  },
  signUp: {
    status: null as string | null,
    isTransferable: false,
    existingSession: null as { sessionId: string } | null,
    create: vi.fn(),
    finalize: vi.fn(),
  },
  setActive: vi.fn(),
}));

vi.mock("@clerk/nextjs", () => ({
  useClerk: () => ({ loaded: true, setActive: mocks.setActive }),
  useSignIn: () => ({ signIn: mocks.signIn }),
  useSignUp: () => ({ signUp: mocks.signUp }),
}));

vi.mock("next/navigation", () => ({
  useRouter: () => ({ replace: mocks.replace }),
  useSearchParams: () => new URLSearchParams(mocks.search.current),
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
            completing: "Completing Google sign-in…",
            invalid_binding: "Invalid desktop binding",
            failed: "Google sign-in failed",
          },
        },
      }),
  }),
}));

import GoogleOAuthCallbackPage from "./page";

describe("GoogleOAuthCallbackPage", () => {
  beforeEach(() => {
    window.history.replaceState(null, "", "/");
    vi.clearAllMocks();
    mocks.search.current = "";
    mocks.signIn.status = "complete";
    mocks.signIn.isTransferable = false;
    mocks.signIn.existingSession = null;
    mocks.signUp.status = null;
    mocks.signUp.isTransferable = false;
    mocks.signUp.existingSession = null;
    mocks.signIn.finalize.mockImplementation(
      async ({ navigate }: { navigate: Navigate }) => {
        await navigate({
          session: { currentTask: null },
          decorateUrl: (url: string) => url,
        });
        return { error: null };
      },
    );
    mocks.signUp.finalize.mockImplementation(
      async ({ navigate }: { navigate: Navigate }) => {
        await navigate({
          session: { currentTask: null },
          decorateUrl: (url: string) => url,
        });
        return { error: null };
      },
    );
  });

  it("keeps an existing Google user on the sign-in flow", async () => {
    const codeChallenge = "a".repeat(43);
    const state = "b".repeat(43);
    mocks.search.current = `platform=desktop&code_challenge=${codeChallenge}&state=${state}`;

    render(<GoogleOAuthCallbackPage />);

    await waitFor(() => expect(mocks.signIn.finalize).toHaveBeenCalledOnce());
    expect(mocks.signUp.create).not.toHaveBeenCalled();
    expect(mocks.signIn.create).not.toHaveBeenCalled();
    expect(mocks.signUp.finalize).not.toHaveBeenCalled();
    expect(mocks.replace).toHaveBeenCalledWith(
      `/login?platform=desktop&code_challenge=${codeChallenge}&state=${state}`,
    );
  });

  it("transfers a first-time Google user from sign-in to sign-up", async () => {
    const codeChallenge = "a".repeat(43);
    const state = "b".repeat(43);
    mocks.search.current = `platform=desktop&code_challenge=${codeChallenge}&state=${state}`;
    mocks.signIn.status = "needs_first_factor";
    mocks.signIn.isTransferable = true;
    mocks.signUp.status = "missing_requirements";
    mocks.signUp.create.mockImplementation(async () => {
      mocks.signUp.status = "complete";
      return { error: null };
    });

    render(<GoogleOAuthCallbackPage />);

    await waitFor(() =>
      expect(mocks.signUp.create).toHaveBeenCalledWith({ transfer: true }),
    );
    await waitFor(() => expect(mocks.signUp.finalize).toHaveBeenCalledOnce());
    expect(mocks.signIn.create).not.toHaveBeenCalled();
    expect(mocks.signIn.finalize).not.toHaveBeenCalled();
    expect(mocks.replace).toHaveBeenCalledWith(
      `/login?platform=desktop&code_challenge=${codeChallenge}&state=${state}`,
    );
  });

  it("preserves a configured broker base path after Clerk finalizes", async () => {
    const codeChallenge = "a".repeat(43);
    const state = "b".repeat(43);
    window.history.replaceState(
      null,
      "",
      "/patchbay/oauth/google/callback",
    );
    mocks.search.current = `platform=desktop&code_challenge=${codeChallenge}&state=${state}`;

    render(<GoogleOAuthCallbackPage />);

    await waitFor(() => expect(mocks.signIn.finalize).toHaveBeenCalledOnce());
    expect(mocks.replace).toHaveBeenCalledWith(
      `/patchbay/login?platform=desktop&code_challenge=${codeChallenge}&state=${state}`,
    );
  });

  it("does not finalize or redirect when the renderer binding is missing", async () => {
    mocks.search.current = `platform=desktop&code_challenge=${"a".repeat(43)}`;

    render(<GoogleOAuthCallbackPage />);

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Invalid desktop binding",
    );
    expect(mocks.signIn.finalize).not.toHaveBeenCalled();
    expect(mocks.replace).not.toHaveBeenCalled();
  });

  it("fails closed when Clerk cannot finalize the session", async () => {
    const codeChallenge = "a".repeat(43);
    const state = "b".repeat(43);
    mocks.search.current = `platform=desktop&code_challenge=${codeChallenge}&state=${state}`;
    mocks.signIn.finalize.mockResolvedValue({ error: new Error("rejected") });

    render(<GoogleOAuthCallbackPage />);

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Google sign-in failed",
    );
    expect(mocks.replace).not.toHaveBeenCalled();
  });
});
