// @vitest-environment jsdom

import { renderToString } from "react-dom/server";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import "@testing-library/jest-dom/vitest";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import Page from "./page";

const STATE = "s".repeat(43);
const CHALLENGE = "c".repeat(43);

const mocks = vi.hoisted(() => ({
  register: vi.fn(),
  complete: vi.fn(),
  getToken: vi.fn(),
  signOut: vi.fn(),
  auth: { isLoaded: true, isSignedIn: false, sessionId: "session-1" },
  searchParams: { current: new URLSearchParams() },
}));

vi.mock("next/navigation", () => ({
  useSearchParams: () => mocks.searchParams.current,
}));

vi.mock("@clerk/nextjs", () => ({
  useAuth: () => ({ ...mocks.auth, getToken: mocks.getToken }),
  useClerk: () => ({ signOut: mocks.signOut }),
}));

vi.mock("@/components/accounts-login-form", () => ({
  AccountsLoginForm: ({ returnUrl }: { returnUrl: string }) => (
    <div data-testid="accounts-login-form" data-return-url={returnUrl} />
  ),
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
    preparing: "Preparing sign-in…",
    brand: "Orvilo",
    quote: "Orvilo quote",
    login: "Login",
    desktopFailed: "The desktop sign-in could not be completed.",
    desktopRestart: "Return to Orvilo and start sign-in again.",
    opening: "Opening Orvilo…",
    finishing: "Finishing sign-in…",
    open: "Open Orvilo",
  }),
}));

const originalFormSubmit = HTMLFormElement.prototype.submit;

beforeEach(() => {
  cleanup();
  document.querySelectorAll("form").forEach((form) => form.remove());
  HTMLFormElement.prototype.submit = originalFormSubmit;
  mocks.auth.isLoaded = true;
  mocks.auth.isSignedIn = false;
  mocks.auth.sessionId = "session-1";
  mocks.register.mockReset().mockResolvedValue(undefined);
  mocks.complete.mockReset();
  mocks.getToken.mockReset();
  mocks.signOut.mockReset().mockResolvedValue(undefined);
  window.sessionStorage.clear();
  mocks.searchParams.current = new URLSearchParams({
    platform: "desktop",
    state: STATE,
    code_challenge: CHALLENGE,
  });
});

afterEach(() => {
  HTMLFormElement.prototype.submit = originalFormSubmit;
});

describe("Accounts desktop login", () => {
  it("registers the desktop attempt without replacing the custom login form", async () => {
    render(<Page />);

    expect(await screen.findByTestId("accounts-login-form")).toHaveAttribute(
      "data-return-url",
      `/login?platform=desktop&state=${STATE}&code_challenge=${CHALLENGE}`,
    );
    expect(screen.queryByText("Opening Orvilo…")).not.toBeInTheDocument();
    await waitFor(() =>
      expect(mocks.register).toHaveBeenCalledWith({
        state: STATE,
        code_challenge: CHALLENGE,
      }),
    );
  });

  it("shows preparation until Clerk has determined the session", () => {
    mocks.auth.isLoaded = false;

    render(<Page />);

    expect(screen.queryByTestId("accounts-login-form")).not.toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent("Preparing sign-in");
    expect(screen.queryByText("Opening Orvilo…")).not.toBeInTheDocument();
  });

  it("renders the custom form for a direct Accounts login without a Desktop binding", async () => {
    mocks.searchParams.current = new URLSearchParams();

    render(<Page />);

    expect(await screen.findByTestId("accounts-login-form")).toHaveAttribute(
      "data-return-url",
      "https://patchbay.aspectlylabs.com/login",
    );
    expect(mocks.register).not.toHaveBeenCalled();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("accepts only the product return target for a direct Accounts login", async () => {
    mocks.searchParams.current = new URLSearchParams({
      return_url: "https://patchbay.aspectlylabs.com/acme/issues",
    });

    render(<Page />);

    expect(await screen.findByTestId("accounts-login-form")).toHaveAttribute(
      "data-return-url",
      "https://patchbay.aspectlylabs.com/acme/issues",
    );
  });

  it("rejects a browser-selected local token destination before requesting a token", async () => {
    mocks.auth.isSignedIn = true;
    mocks.searchParams.current.set("session_api", "http://localhost:19080");
    render(<Page />);
    expect(await screen.findByRole("alert")).toBeInTheDocument();
    expect(mocks.getToken).not.toHaveBeenCalled();
    expect(document.querySelector('input[name="session"]')).toBeNull();
  });

  it("completes a local login through the broker without posting a bearer to localhost", async () => {
    window.sessionStorage.setItem(`patchbay_desktop_attempt:${STATE}`, CHALLENGE);
    mocks.auth.isSignedIn = true;
    mocks.getToken.mockResolvedValue("clerk-session-token");
    mocks.complete.mockResolvedValue({ code: `pbl_${"c".repeat(43)}`, callbackProtocol: "patchbay" });
    mocks.searchParams.current.set("session_mode", "local");
    render(<Page />);
    await waitFor(() => expect(mocks.complete).toHaveBeenCalledWith("clerk-session-token", {
      state: STATE, code_challenge: CHALLENGE, local: true,
    }));
    const link = await screen.findByRole("link", { name: "Open Orvilo" });
    expect(link.getAttribute("href")).toContain("patchbay://auth/callback?code=pbl_");
    expect(document.querySelector("form")).toBeNull();
    expect(document.querySelector('input[name="session"]')).toBeNull();
  });

  it("prepares an existing session before showing the form, without attempting completion or redirecting", async () => {
    mocks.auth.isSignedIn = true;
    mocks.signOut.mockImplementation(async () => { mocks.auth.isSignedIn = false; mocks.auth.sessionId = ""; });
    render(<Page />);
    expect(screen.getByRole("status")).toHaveTextContent("Preparing sign-in");
    expect(await screen.findByTestId("accounts-login-form")).toBeInTheDocument();
    expect(mocks.register).toHaveBeenCalledOnce();
    expect(mocks.signOut).toHaveBeenCalledWith(expect.any(Function), { sessionId: "session-1" });
    expect(mocks.complete).not.toHaveBeenCalled();
  });

  it("does not expose the form or complete before registration finishes", async () => {
    let resolveRegister: ((value: undefined) => void) | undefined;
    mocks.register.mockReturnValue(new Promise((resolve) => { resolveRegister = resolve; }));
    render(<Page />);
    await waitFor(() => expect(mocks.register).toHaveBeenCalledOnce());
    expect(screen.queryByTestId("accounts-login-form")).not.toBeInTheDocument();
    expect(mocks.complete).not.toHaveBeenCalled();
    resolveRegister?.(undefined);
    expect(await screen.findByTestId("accounts-login-form")).toBeInTheDocument();
  });

  it("shows a rejected completion without signing out and looping back to the form", async () => {
    window.sessionStorage.setItem(`patchbay_desktop_attempt:${STATE}`, CHALLENGE);
    mocks.auth.isSignedIn = true;
    mocks.getToken.mockResolvedValue("token");
    mocks.complete.mockRejectedValue(new Error("rejected"));
    render(<Page />);
    expect(await screen.findByRole("alert")).toBeInTheDocument();
    expect(mocks.signOut).not.toHaveBeenCalled();
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
    expect(screen.getByRole("alert")).toHaveTextContent("Return to Orvilo");
    expect(screen.queryByTestId("accounts-login-form")).not.toBeInTheDocument();
  });

  it("keeps a clickable desktop callback when production complete cannot auto-open the app", async () => {
    window.sessionStorage.setItem(`patchbay_desktop_attempt:${STATE}`, CHALLENGE);
    mocks.auth.isSignedIn = true;
    mocks.getToken.mockResolvedValue("clerk-session-token");
    mocks.complete.mockResolvedValue({
      code: `pbd_${"c".repeat(43)}`,
      callbackProtocol: "patchbay",
    });

    render(<Page />);

    expect(
      await screen.findByRole("link", { name: "Open Orvilo" }),
    ).toHaveAttribute(
      "href",
      `patchbay://auth/callback?code=pbd_${"c".repeat(43)}&state=${STATE}`,
    );
    expect(screen.getByText("Finishing sign-in…")).toBeInTheDocument();
    expect(screen.queryByText("Opening Orvilo…")).not.toBeInTheDocument();
  });
});

it("waits for the session notification after signOut resolves, without exposing a stale signed-in form", async () => {
  mocks.auth.isSignedIn = true;
  const { rerender } = render(<Page />);
  await waitFor(() => expect(mocks.signOut).toHaveBeenCalledOnce());
  expect(screen.queryByTestId("accounts-login-form")).not.toBeInTheDocument();
  expect(mocks.complete).not.toHaveBeenCalled();
  mocks.auth.isSignedIn = false;
  mocks.auth.sessionId = "";
  rerender(<Page />);
  expect(await screen.findByTestId("accounts-login-form")).toBeInTheDocument();
});

it.each([true, false])("never paints an account form for a signed-in session before effects run (desktop=%s)", (desktop) => {
  mocks.auth.isSignedIn = true;
  if (!desktop) mocks.searchParams.current = new URLSearchParams();
  const html = renderToString(<Page />);
  expect(html).not.toContain('data-testid="accounts-login-form"');
  expect(html).toContain('role="status"');
});
