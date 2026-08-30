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

});
