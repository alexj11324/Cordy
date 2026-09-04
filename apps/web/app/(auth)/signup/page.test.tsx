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

import SignUpPage from "./page";

describe("SignUpPage", () => {
  beforeEach(() => {
    search.current = "";
    signUpProps.current = {};
  });

  it("renders the canonical signup flow without desktop handoff state", () => {
    render(<SignUpPage />);

    expect(screen.getByTestId("clerk-sign-up")).toBeInTheDocument();
    expect(signUpProps.current).toMatchObject({
      routing: "path",
      path: "/signup",
      signInUrl: "/login",
      fallbackRedirectUrl: "/",
    });
  });

  it("preserves the desktop handoff through signup and sign-in", () => {
    search.current =
      "platform=desktop&code_challenge=challenge-value&state=opaque-state";

    render(<SignUpPage />);

    expect(signUpProps.current).toMatchObject({
      signInUrl:
        "/login?platform=desktop&code_challenge=challenge-value&state=opaque-state",
      fallbackRedirectUrl:
        "/login?platform=desktop&code_challenge=challenge-value&state=opaque-state",
    });
  });

  it("preserves a validated web redirect through signup and sign-in", () => {
    search.current = "redirect_url=%2Fusage%3Ftab%3Dbilling%23summary";

    render(<SignUpPage />);

    expect(signUpProps.current).toMatchObject({
      signInUrl: "/login?redirect_url=%2Fusage%3Ftab%3Dbilling%23summary",
      fallbackRedirectUrl: "/usage?tab=billing#summary",
    });
  });

  it("rejects an external web redirect", () => {
    search.current = "redirect_url=https%3A%2F%2Fevil.example%2Ftakeover";

    render(<SignUpPage />);

    expect(signUpProps.current).toMatchObject({
      signInUrl: "/login",
      fallbackRedirectUrl: "/",
    });
  });
});
