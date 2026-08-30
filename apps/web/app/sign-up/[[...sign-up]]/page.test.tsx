import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const { search, signUpProps } = vi.hoisted(() => ({
  search: { current: "" },
  signUpProps: { current: {} as Record<string, unknown> },
}));

vi.mock("@clerk/nextjs", () => ({
  SignUp: (props: Record<string, unknown>) => {
    signUpProps.current = props;
    return <div data-testid="clerk-sign-up" />;
  },
}));

vi.mock("next/navigation", () => ({
  useSearchParams: () => new URLSearchParams(search.current),
}));

vi.mock("@patchbay/views/i18n", () => ({
  useT: () => ({
    t: () => "Invalid desktop app origin.",
  }),
}));

import SignUpPage from "./page";

describe("SignUpPage (sign-up route)", () => {
  beforeEach(() => {
    search.current = "";
    signUpProps.current = {};
    delete process.env.NEXT_PUBLIC_DESKTOP_APP_ORIGIN;
  });

  it("preserves a validated web target when switching to sign-in", () => {
    search.current = "redirect_url=%2Fusage%3Ftab%3Dbilling%23summary";

    render(<SignUpPage />);

    expect(signUpProps.current).toMatchObject({
      signInUrl: "/sign-in?redirect_url=%2Fusage%3Ftab%3Dbilling%23summary",
      fallbackRedirectUrl: "/usage?tab=billing#summary",
    });
  });

  it("preserves the allowlisted browser app origin with desktop PKCE state", () => {
    process.env.NEXT_PUBLIC_DESKTOP_APP_ORIGIN = "https://patchbay.aspectlylabs.com";
    search.current =
      "platform=desktop&code_challenge=challenge-value&state=opaque-state" +
      "&app_origin=https%3A%2F%2Fpatchbay.aspectlylabs.com";

    render(<SignUpPage />);

    const query =
      "platform=desktop&code_challenge=challenge-value&state=opaque-state" +
      "&app_origin=https%3A%2F%2Fpatchbay.aspectlylabs.com";
    expect(signUpProps.current).toMatchObject({
      signInUrl: `/sign-in?${query}`,
      fallbackRedirectUrl: `/login?${query}`,
    });
  });

  it("preserves the desktop handoff through the alternate signup route", () => {
    search.current =
      "platform=desktop&code_challenge=challenge-value&state=opaque-state";

    render(<SignUpPage />);

    expect(screen.getByTestId("clerk-sign-up")).toBeInTheDocument();
    expect(signUpProps.current).toMatchObject({
      path: "/sign-up",
      signInUrl:
        "/sign-in?platform=desktop&code_challenge=challenge-value&state=opaque-state",
      fallbackRedirectUrl:
        "/login?platform=desktop&code_challenge=challenge-value&state=opaque-state",
    });
  });
});
