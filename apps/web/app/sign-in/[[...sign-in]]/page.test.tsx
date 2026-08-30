import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const { search, signInProps } = vi.hoisted(() => ({
  search: { current: "" },
  signInProps: { current: {} as Record<string, unknown> },
}));

vi.mock("@clerk/nextjs", () => ({
  SignIn: (props: Record<string, unknown>) => {
    signInProps.current = props;
    return <div data-testid="clerk-sign-in" />;
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

import SignInPage from "./page";

describe("SignInPage (sign-in route)", () => {
  beforeEach(() => {
    search.current = "";
    signInProps.current = {};
    delete process.env.NEXT_PUBLIC_DESKTOP_APP_ORIGIN;
  });

  it("preserves a validated web target when switching to sign-up", () => {
    search.current = "redirect_url=%2Fusage%3Ftab%3Dbilling%23summary";

    render(<SignInPage />);

    expect(screen.getByTestId("clerk-sign-in")).toBeInTheDocument();
    expect(signInProps.current).toMatchObject({
      signUpUrl: "/sign-up?redirect_url=%2Fusage%3Ftab%3Dbilling%23summary",
      fallbackRedirectUrl: "/usage?tab=billing#summary",
    });
  });

  it("preserves PKCE, state, and an allowlisted browser app origin", () => {
    process.env.NEXT_PUBLIC_DESKTOP_APP_ORIGIN = "https://patchbay.aspectlylabs.com";
    search.current =
      "platform=desktop&code_challenge=challenge-value&state=opaque-state" +
      "&app_origin=https%3A%2F%2Fpatchbay.aspectlylabs.com";

    render(<SignInPage />);

    const query =
      "platform=desktop&code_challenge=challenge-value&state=opaque-state" +
      "&app_origin=https%3A%2F%2Fpatchbay.aspectlylabs.com";
    expect(signInProps.current).toMatchObject({
      signUpUrl: `/sign-up?${query}`,
      fallbackRedirectUrl: `/login?${query}`,
    });
  });

  it("fails closed for a mismatched browser app origin", () => {
    process.env.NEXT_PUBLIC_DESKTOP_APP_ORIGIN = "https://patchbay.aspectlylabs.com";
    search.current =
      "platform=desktop&code_challenge=challenge-value&state=opaque-state" +
      "&app_origin=https%3A%2F%2Fevil.example";

    render(<SignInPage />);

    expect(screen.getByRole("alert")).toHaveTextContent(
      "Invalid desktop app origin.",
    );
    expect(screen.queryByTestId("clerk-sign-in")).not.toBeInTheDocument();
  });
});
