import { render } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const { search, callbackProps } = vi.hoisted(() => ({
  search: { current: "" },
  callbackProps: { current: {} as Record<string, unknown> },
}));

vi.mock("@clerk/nextjs", () => ({
  AuthenticateWithRedirectCallback: (props: Record<string, unknown>) => {
    callbackProps.current = props;
    return null;
  },
}));

vi.mock("next/navigation", () => ({
  useSearchParams: () => new URLSearchParams(search.current),
}));

import SSOCallbackPage from "./page";

describe("SSOCallbackPage", () => {
  beforeEach(() => {
    search.current = "";
    callbackProps.current = {};
    delete process.env.NEXT_PUBLIC_DESKTOP_APP_ORIGIN;
  });

  it("retains a validated web redirect across Clerk fallback routes", () => {
    search.current = "redirect_url=%2Fusage%3Ftab%3Dbilling%23summary";

    render(<SSOCallbackPage />);

    expect(callbackProps.current).toMatchObject({
      signInUrl: "/sign-in?redirect_url=%2Fusage%3Ftab%3Dbilling%23summary",
      signUpUrl: "/sign-up?redirect_url=%2Fusage%3Ftab%3Dbilling%23summary",
      signInFallbackRedirectUrl: "/usage?tab=billing#summary",
      signUpFallbackRedirectUrl: "/usage?tab=billing#summary",
    });
  });

  it("retains PKCE, state, and the allowlisted browser app origin", () => {
    process.env.NEXT_PUBLIC_DESKTOP_APP_ORIGIN = "https://patchbay.aspectlylabs.com";
    search.current =
      "platform=desktop&code_challenge=challenge-value&state=opaque-state" +
      "&app_origin=https%3A%2F%2Fpatchbay.aspectlylabs.com";

    render(<SSOCallbackPage />);

    const query =
      "platform=desktop&code_challenge=challenge-value&state=opaque-state" +
      "&app_origin=https%3A%2F%2Fpatchbay.aspectlylabs.com";
    expect(callbackProps.current).toMatchObject({
      signInUrl: `/sign-in?${query}`,
      signUpUrl: `/sign-up?${query}`,
      signInFallbackRedirectUrl: `/login?${query}`,
      signUpFallbackRedirectUrl: `/login?${query}`,
    });
  });
});
