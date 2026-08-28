import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const { signInProps } = vi.hoisted(() => ({
  signInProps: { current: {} as Record<string, unknown> },
}));

vi.mock("@clerk/nextjs", () => ({
  SignIn: (props: Record<string, unknown>) => {
    signInProps.current = props;
    return <div data-testid="clerk-sign-in" />;
  },
}));

import LoginPage from "./page";

describe("LoginPage", () => {
  beforeEach(() => {
    signInProps.current = {};
  });

  it("renders the Clerk sign-in flow at the canonical login route", () => {
    render(<LoginPage />);

    expect(screen.getByTestId("clerk-sign-in")).toBeInTheDocument();
    expect(signInProps.current).toMatchObject({
      routing: "path",
      path: "/login",
      signUpUrl: "/signup",
      forceRedirectUrl: "/",
    });
  });
});
