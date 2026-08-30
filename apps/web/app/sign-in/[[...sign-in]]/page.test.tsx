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

import SignInPage from "./page";

describe("SignInPage (sign-in route)", () => {
  beforeEach(() => {
    search.current = "";
    signInProps.current = {};
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

});
