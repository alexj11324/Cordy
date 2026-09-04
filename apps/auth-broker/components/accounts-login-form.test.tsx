// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import "@testing-library/jest-dom/vitest";
import { beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  signIn: {
    create: vi.fn(),
    emailCode: {
      sendCode: vi.fn(),
      verifyCode: vi.fn(),
    },
    finalize: vi.fn(),
    reset: vi.fn(),
    status: "needs_first_factor" as string,
  },
  signUp: {
    create: vi.fn(),
    update: vi.fn(),
  },
  setActive: vi.fn(),
}));

vi.mock("@clerk/nextjs", () => ({
  useSignIn: () => ({
    signIn: mocks.signIn,
    setActive: mocks.setActive,
  }),
  useSignUp: () => ({
    signUp: mocks.signUp,
    setActive: mocks.setActive,
  }),
}));

vi.mock("@/lib/auth-messages", () => ({
  useAuthMessages: () => ({
    createAccount: "Create an account",
    emailDescription: "Enter your email below to create your account",
    emailPlaceholder: "name@example.com",
    emailButton: "Sign In with Email",
    continueWith: "Or continue with",
    google: "Continue with Google",
    terms: "By clicking continue, you agree to our Terms of Service and Privacy Policy.",
    verifyTitle: "Verify your email",
    verifyDescription: "Enter the verification code sent to your email",
    verificationCode: "Verification code",
    verifyButton: "Verify",
    back: "Back",
    startOver: "Start over",
    completeAccount: "Complete your account",
    completeAccountDescription: "Your email has been verified. Finish creating your account.",
    createAccountButton: "Create account",
    legal: "I agree to the Terms of Service and Privacy Policy",
    unavailable: "Authentication is unavailable.",
  }),
}));

import {
  AccountsLoginForm,
  buildGoogleLoginUrl,
} from "./accounts-login-form";

beforeEach(() => {
  cleanup();
  mocks.signIn.create.mockReset();
  mocks.signIn.emailCode.sendCode.mockReset().mockResolvedValue({ error: null });
  mocks.signIn.emailCode.verifyCode.mockReset().mockResolvedValue({ error: null });
  mocks.signIn.finalize.mockReset().mockResolvedValue({ error: null });
  mocks.signIn.reset.mockReset();
  mocks.signIn.status = "needs_first_factor";
  mocks.signUp.create.mockReset();
  mocks.signUp.update.mockReset();
  mocks.setActive.mockReset().mockResolvedValue(undefined);
});

describe("AccountsLoginForm", () => {
  it("keeps the standalone product return target on the Google broker route", () => {
    expect(
      buildGoogleLoginUrl(
        "https://patchbay.aspectlylabs.com/login",
        "https://accounts.aspectlylabs.com",
      ),
    ).toBe(
      "https://accounts.aspectlylabs.com/oauth/google?return_url=https%3A%2F%2Fpatchbay.aspectlylabs.com%2Flogin",
    );
  });

  it("keeps the Desktop binding query on the Google broker route", () => {
    expect(
      buildGoogleLoginUrl(
        "/login?platform=desktop&state=state&code_challenge=challenge",
        "https://accounts.aspectlylabs.com",
      ),
    ).toBe(
      "https://accounts.aspectlylabs.com/oauth/google?platform=desktop&state=state&code_challenge=challenge",
    );
  });

  it("renders the cardless shadcn authentication controls", () => {
    render(<AccountsLoginForm returnUrl="/login" />);

    expect(
      screen.getByRole("heading", { name: "Create an account" }),
    ).toBeInTheDocument();
    expect(screen.getByPlaceholderText("name@example.com")).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Sign In with Email" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Continue with Google" }),
    ).toBeInTheDocument();
    expect(screen.getByText("Or continue with")).toBeInTheDocument();
    expect(screen.getByText(/Terms of Service/)).toBeInTheDocument();
    expect(document.querySelector('[data-slot="card"]')).not.toBeInTheDocument();
  });

  it("starts Clerk email-code verification without rendering a Clerk card", async () => {
    mocks.signIn.create.mockResolvedValue({ error: null });

    render(<AccountsLoginForm returnUrl="/login" />);
    const email = screen.getByPlaceholderText("name@example.com");
    fireEvent.change(email, { target: { value: "person@example.com" } });
    fireEvent.click(screen.getByRole("button", { name: "Sign In with Email" }));

    await waitFor(() =>
      expect(mocks.signIn.create).toHaveBeenCalledWith({
        identifier: "person@example.com",
        signUpIfMissing: true,
      }),
    );
    expect(mocks.signIn.emailCode.sendCode).toHaveBeenCalledOnce();
    expect(screen.getByLabelText("Verification code")).toBeInTheDocument();
  });

  it("activates the Clerk session after verifying the email code", async () => {
    mocks.signIn.create.mockResolvedValue({ error: null });
    mocks.signIn.emailCode.verifyCode.mockImplementation(async () => {
      mocks.signIn.status = "complete";
      return { error: null };
    });

    render(<AccountsLoginForm returnUrl="/login" />);
    fireEvent.change(screen.getByPlaceholderText("name@example.com"), {
      target: { value: "person@example.com" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Sign In with Email" }));
    await screen.findByLabelText("Verification code");
    fireEvent.change(screen.getByLabelText("Verification code"), {
      target: { value: "123456" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Verify" }));

    await waitFor(() =>
      expect(mocks.signIn.emailCode.verifyCode).toHaveBeenCalledWith({
        code: "123456",
      }),
    );
    expect(mocks.signIn.finalize).toHaveBeenCalledOnce();
  });
});
