import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
const { sso, staleSso, client } = vi.hoisted(() => {
  const sso = vi.fn().mockResolvedValue({});
  const staleSso = vi.fn();
  const client = { signIn: { __internal_future: { sso: staleSso } }, resetSignIn: vi.fn() };
  client.resetSignIn.mockImplementation(() => { client.signIn = { __internal_future: { sso } }; });
  return { sso, staleSso, client };
});
vi.mock("@clerk/nextjs", () => ({ useClerk: () => ({ client }) }));
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
    expect(client.resetSignIn).toHaveBeenCalledOnce();
    expect(staleSso).not.toHaveBeenCalled();
    await waitFor(() => expect(sso).toHaveBeenCalledWith({
      strategy: "oauth_google",
      redirectCallbackUrl: "/sso-callback?redirect_url=%2Fusage%3Ftab%3Dbilling%23summary",
      redirectUrl: "/login?redirect_url=%2Fusage%3Ftab%3Dbilling%23summary",
    }));
  });
});
