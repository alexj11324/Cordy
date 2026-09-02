// @vitest-environment jsdom

import { StrictMode, type ReactNode } from "react";
import { describe, expect, it, beforeEach, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { I18nProvider } from "@patchbay/core/i18n/react";
import enCommon from "../locales/en/common.json";

const authState = vi.hoisted(() => ({
  user: null as { id: string } | null,
  isLoading: false,
}));
const mockRedeem = vi.hoisted(() => vi.fn());
const mockPush = vi.hoisted(() => vi.fn());
const MockApiError = vi.hoisted(() =>
  class MockApiError extends Error {
    readonly status: number;
    readonly statusText: string;

    constructor(message: string, status: number, statusText = "") {
      super(message);
      this.name = "ApiError";
      this.status = status;
      this.statusText = statusText;
    }
  },
);

vi.mock("@patchbay/core/auth", () => {
  const useAuthStore = Object.assign(
    (selector?: (state: typeof authState) => unknown) =>
      selector ? selector(authState) : authState,
    { getState: () => authState },
  );
  return { useAuthStore };
});

vi.mock("@patchbay/core/api", () => ({
  api: { redeemWeixinBindingToken: mockRedeem },
  ApiError: MockApiError,
}));

vi.mock("../navigation/context", () => ({
  useNavigation: () => ({ push: mockPush }),
  useOptionalNavigation: () => ({ push: mockPush }),
}));

import { WeixinBindPage } from "./bind-page";

const resources = { en: { common: enCommon } };

function Wrapper({ children }: { children: ReactNode }) {
  return (
    <I18nProvider locale="en" resources={resources}>
      {children}
    </I18nProvider>
  );
}

function renderPage(token: string | null) {
  return render(<WeixinBindPage token={token} />, { wrapper: Wrapper });
}

describe("WeixinBindPage", () => {
  beforeEach(() => {
    authState.user = null;
    authState.isLoading = false;
    mockRedeem.mockReset();
    mockPush.mockReset();
  });

  it("requires sign-in before redeeming a token", () => {
    renderPage("token-1");
    expect(screen.getByRole("button", { name: /sign in/i })).toBeInTheDocument();
    expect(mockRedeem).not.toHaveBeenCalled();
  });

  it("preserves the token in the sign-in return path", () => {
    renderPage("token/1");
    fireEvent.click(screen.getByRole("button", { name: /sign in/i }));
    expect(mockPush).toHaveBeenCalledTimes(1);
    const signInURL = new URL(mockPush.mock.calls[0]?.[0] as string, "https://app.example.test");
    expect(signInURL.searchParams.get("next")).toBe("/weixin/bind?token=token%2F1");
  });

  it("redeems for an authenticated user and shows the success state", async () => {
    authState.user = { id: "user-1" };
    mockRedeem.mockResolvedValue({
      workspace_id: "workspace-1",
      installation_id: "installation-1",
      weixin_user_id: "wx-user-1",
    });
    renderPage("token-1");
    await waitFor(() => expect(mockRedeem).toHaveBeenCalledWith("token-1"));
    expect(await screen.findByText(/you're linked/i)).toBeInTheDocument();
  });

  it("maps an expired token to actionable localized copy", async () => {
    authState.user = { id: "user-1" };
    mockRedeem.mockRejectedValue(new MockApiError("binding token expired", 410));
    renderPage("token-1");
    expect(await screen.findByText(/invalid or expired/i)).toBeInTheDocument();
  });

  it("maps the Go handler's already-bound response", async () => {
    authState.user = { id: "user-1" };
    mockRedeem.mockRejectedValue(new MockApiError("binding token already assigned", 409));
    renderPage("token-1");
    expect(await screen.findByText(/already linked/i)).toBeInTheDocument();
  });

  it("does not claim success for a malformed fallback response", async () => {
    authState.user = { id: "user-1" };
    mockRedeem.mockResolvedValue({ workspace_id: "", installation_id: "", weixin_user_id: "" });
    renderPage("token-1");
    expect(await screen.findByText(/couldn't complete/i)).toBeInTheDocument();
    expect(screen.queryByText(/you're linked/i)).toBeNull();
  });

  it("redeems a token only once when effects are replayed", async () => {
    authState.user = { id: "user-1" };
    mockRedeem.mockResolvedValue({
      workspace_id: "workspace-1",
      installation_id: "installation-1",
      weixin_user_id: "wx-user-1",
    });
    render(
      <StrictMode>
        <Wrapper>
          <WeixinBindPage token="token-1" />
        </Wrapper>
      </StrictMode>,
    );
    await waitFor(() => expect(mockRedeem).toHaveBeenCalledTimes(1));
  });
});
