import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const { signInProps, authState, search } = vi.hoisted(() => ({
  signInProps: { current: {} as Record<string, unknown> },
  authState: {
    current: { isLoaded: true, isSignedIn: false, getToken: vi.fn() },
  },
  search: { current: "" },
}));

vi.mock("@clerk/nextjs", () => ({
  SignIn: (props: Record<string, unknown>) => {
    signInProps.current = props;
    return <div data-testid="clerk-sign-in" />;
  },
  useAuth: () => authState.current,
}));

vi.mock("next/navigation", () => ({
  useSearchParams: () => new URLSearchParams(search.current),
}));

import LoginPage from "./page";

describe("LoginPage", () => {
  beforeEach(() => {
    signInProps.current = {};
    search.current = "";
    authState.current = { isLoaded: true, isSignedIn: false, getToken: vi.fn() };
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

  it("preserves a validated CLI callback through Clerk sign-in", () => {
    search.current =
      "cli_callback=http%3A%2F%2F127.0.0.1%3A43821%2Fcallback&cli_state=opaque-state";

    render(<LoginPage />);

    expect(signInProps.current.forceRedirectUrl).toBe(
      "/login?cli_callback=http%3A%2F%2F127.0.0.1%3A43821%2Fcallback&cli_state=opaque-state",
    );
  });

  it("offers CLI authorization after Clerk has established the session", () => {
    search.current =
      "cli_callback=http%3A%2F%2Flocalhost%3A43821%2Fcallback&cli_state=opaque-state";
    authState.current = { isLoaded: true, isSignedIn: true, getToken: vi.fn() };

    render(<LoginPage />);

    expect(screen.getByRole("button", { name: "Authorize CLI" })).toBeInTheDocument();
    expect(screen.queryByTestId("clerk-sign-in")).not.toBeInTheDocument();
  });
});
