import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
const { sso } = vi.hoisted(() => ({ sso: vi.fn().mockResolvedValue({}) }));
vi.mock("@clerk/nextjs", () => ({ useSignIn: () => ({ signIn: { sso } }) }));
vi.mock("@patchbay/views/i18n", () => ({ useLocale: () => "en" }));
vi.mock("@patchbay/auth-ui/login-form", () => ({
  AccountsLoginForm: ({ onGoogleLogin }: { onGoogleLogin: () => Promise<void> }) =>
    <button onClick={() => void onGoogleLogin()}>Google</button>,
}));
import { WebAccountsLoginForm } from "./accounts-login-form";
describe("Web custom form OAuth handoff", () => {
  it("keeps Google on the Web session boundary and preserves the return destination", async () => {
    render(<WebAccountsLoginForm returnUrl="/usage?tab=billing#summary" />);
    fireEvent.click(screen.getByRole("button", { name: "Google" }));
    await waitFor(() => expect(sso).toHaveBeenCalledWith({
      strategy: "oauth_google",
      redirectCallbackUrl: "/sso-callback?redirect_url=%2Fusage%3Ftab%3Dbilling%23summary",
      redirectUrl: "/login?redirect_url=%2Fusage%3Ftab%3Dbilling%23summary",
    }));
  });
});
