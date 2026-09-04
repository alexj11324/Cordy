// @vitest-environment jsdom

import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nProvider } from "@patchbay/core/i18n/react";
import enCommon from "../../locales/en/common.json";
import enSettings from "../../locales/en/settings.json";

const mockUpdateMe = vi.hoisted(() => vi.fn());
const mockSetUser = vi.hoisted(() => vi.fn());
const mockToastSuccess = vi.hoisted(() => vi.fn());
const mockToastError = vi.hoisted(() => vi.fn());
const userRef = vi.hoisted(() => ({
  current: {
    id: "user-1",
    name: "Ada",
    profile_description: "Builds compilers",
    avatar_url: null as string | null,
  },
}));

vi.mock("@patchbay/core/api", () => ({
  api: { updateMe: mockUpdateMe },
}));

vi.mock("sonner", () => ({
  toast: { success: mockToastSuccess, error: mockToastError },
}));

vi.mock("@patchbay/core/auth", async () => {
  const actual =
    await vi.importActual<typeof import("@patchbay/core/auth")>(
      "@patchbay/core/auth",
    );
  type AuthState = {
    user: typeof userRef.current;
    setUser: typeof mockSetUser;
  };
  const state = (): AuthState => ({
    user: userRef.current,
    setUser: mockSetUser,
  });
  const useAuthStore = Object.assign(
    (sel?: (s: AuthState) => unknown) => (sel ? sel(state()) : state()),
    { getState: state },
  );
  return { ...actual, useAuthStore };
});

vi.mock("../../common/avatar-upload-control", () => ({
  AvatarUploadControl: () => <div data-testid="profile-avatar" />,
}));

import { AccountTab } from "./account-tab";

const TEST_RESOURCES = {
  en: { common: enCommon, settings: enSettings },
};

function I18nWrapper({ children }: { children: ReactNode }) {
  return (
    <I18nProvider locale="en" resources={TEST_RESOURCES}>
      {children}
    </I18nProvider>
  );
}

describe("AccountTab", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    userRef.current = {
      id: "user-1",
      name: "Ada",
      profile_description: "Builds compilers",
      avatar_url: null,
    };
  });

  afterEach(() => {
    cleanup();
  });

  it("keeps avatar, name, and about — the same profile fields as before", () => {
    render(<AccountTab />, { wrapper: I18nWrapper });

    expect(screen.getByTestId("profile-avatar")).toBeInTheDocument();
    expect(screen.getByTestId("profile-display-name-value")).toHaveTextContent(
      "Ada",
    );
    expect(screen.getByTestId("profile-about-value")).toHaveTextContent(
      "Builds compilers",
    );
    expect(
      screen.getByText(/Shared with agents working on your behalf/),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("textbox", { name: "Name" }),
    ).not.toBeInTheDocument();
  });

  it("opens the same fields for in-place editing", async () => {
    const user = userEvent.setup();
    render(<AccountTab />, { wrapper: I18nWrapper });

    await user.click(screen.getByRole("button", { name: "Edit profile info" }));

    expect(screen.getByRole("textbox", { name: "Name" })).toHaveValue("Ada");
    expect(screen.getByRole("textbox", { name: "About you" })).toHaveValue(
      "Builds compilers",
    );
    expect(screen.getByText(/\/2000$/)).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Done editing profile info" }),
    ).toBeInTheDocument();
  });

  it("saves on Done when the draft changed", async () => {
    mockUpdateMe.mockResolvedValueOnce({
      ...userRef.current,
      name: "Ada Lovelace",
    });
    const user = userEvent.setup();
    render(<AccountTab />, { wrapper: I18nWrapper });

    await user.click(screen.getByRole("button", { name: "Edit profile info" }));
    await user.clear(screen.getByRole("textbox", { name: "Name" }));
    await user.type(screen.getByRole("textbox", { name: "Name" }), "Ada Lovelace");
    await user.click(
      screen.getByRole("button", { name: "Done editing profile info" }),
    );

    await waitFor(() => {
      expect(mockUpdateMe).toHaveBeenCalledWith({
        name: "Ada Lovelace",
        profile_description: "Builds compilers",
      });
    });
  });
});
