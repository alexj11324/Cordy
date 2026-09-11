// @vitest-environment jsdom

import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nProvider, LocaleAdapterProvider } from "@orvilo/core/i18n/react";
import enCommon from "../../locales/en/common.json";
import enSettings from "../../locales/en/settings.json";

const mockUpdateMe = vi.hoisted(() => vi.fn());
const mockRequestEmailChange = vi.hoisted(() => vi.fn());
const mockConfirmEmailChange = vi.hoisted(() => vi.fn());
const mockSetUser = vi.hoisted(() => vi.fn());
const mockToastSuccess = vi.hoisted(() => vi.fn());
const mockToastError = vi.hoisted(() => vi.fn());
const userRef = vi.hoisted(() => ({
  current: {
    id: "user-1",
    is_guest: false,
    name: "Ada",
    email: "ada@example.com",
    profile_description: "Builds compilers",
    avatar_url: null as string | null,
    language: "en",
    timezone: "America/New_York",
    profile_details: {
      first_name: "Ada",
      last_name: "Lovelace",
      preferred_name: "Ada",
      username: "ada",
      role: "product-ops",
      phone: "+12065551243",
      website: "ada.example.com",
      start_week: "monday",
      time_format: "24-hour",
    },
  },
}));

vi.mock("@orvilo/core/api", () => ({
  api: {
    updateMe: mockUpdateMe,
    requestEmailChange: mockRequestEmailChange,
  },
}));

vi.mock("sonner", () => ({
  toast: { success: mockToastSuccess, error: mockToastError },
}));

vi.mock("@orvilo/core/auth", async () => {
  const actual =
    await vi.importActual<typeof import("@orvilo/core/auth")>(
      "@orvilo/core/auth",
    );
  type AuthState = {
    user: typeof userRef.current;
    setUser: typeof mockSetUser;
    confirmEmailChange: typeof mockConfirmEmailChange;
  };
  const state = (): AuthState => ({
    user: userRef.current,
    setUser: mockSetUser,
    confirmEmailChange: mockConfirmEmailChange,
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
    <LocaleAdapterProvider
      adapter={{
        getUserChoice: () => "en",
        getSystemPreferences: () => ["en"],
        persist: vi.fn(),
      }}
    >
      <I18nProvider locale="en" resources={TEST_RESOURCES}>
        {children}
      </I18nProvider>
    </LocaleAdapterProvider>
  );
}

describe("AccountTab", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    userRef.current = {
      id: "user-1",
      is_guest: false,
      name: "Ada",
      email: "ada@example.com",
      profile_description: "Builds compilers",
      avatar_url: null,
      language: "en",
      timezone: "America/New_York",
      profile_details: {
        first_name: "Ada",
        last_name: "Lovelace",
        preferred_name: "Ada",
        username: "ada",
        role: "product-ops",
        phone: "+12065551243",
        website: "ada.example.com",
        start_week: "monday",
        time_format: "24-hour",
      },
    };
    mockUpdateMe.mockResolvedValue({ ...userRef.current });
    mockRequestEmailChange.mockResolvedValue(undefined);
    mockConfirmEmailChange.mockResolvedValue({
      ...userRef.current,
      email: "new@example.com",
    });
  });

  afterEach(() => {
    cleanup();
  });

  it("renders the profile3 card with the real account fields", () => {
    render(<AccountTab />, { wrapper: I18nWrapper });

    expect(screen.getByTestId("profile-avatar")).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "Profile" })).not.toBeInTheDocument();
    expect(screen.queryByText("Profile updates are shared")).not.toBeInTheDocument();
    expect(screen.getByLabelText("First Name")).toHaveValue("Ada");
    expect(screen.getByLabelText("Last Name")).toHaveValue("Lovelace");
    expect(screen.getByLabelText("Primary Email Address")).toHaveValue(
      "ada@example.com",
    );
    expect(screen.getByLabelText("Preferred Name")).toHaveValue("Ada");
    expect(screen.getByLabelText("About You")).toHaveValue("Builds compilers");
    expect(screen.getByLabelText("Username")).toHaveValue("ada");
    expect(screen.getByLabelText("Phone Number")).toHaveValue(
      "+1 206 555 1243",
    );
    expect(screen.getByLabelText("Website")).toHaveValue("ada.example.com");
    expect(screen.getByRole("combobox", { name: "Role" })).toBeInTheDocument();
    expect(
      screen.getByRole("combobox", { name: "Preferred Timezone" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("combobox", { name: "Start Week On" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("combobox", { name: "Language" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("combobox", { name: "Time Format" }),
    ).toBeInTheDocument();
  });

  it("changes email through the request and verification flow", async () => {
    const user = userEvent.setup();
    render(<AccountTab />, { wrapper: I18nWrapper });

    await user.click(screen.getByRole("button", { name: "Edit" }));
    const email = screen.getByLabelText("New email address");
    await user.clear(email);
    await user.type(email, "new@example.com");
    await user.click(screen.getByRole("button", { name: "Send code" }));

    await waitFor(() => {
      expect(mockRequestEmailChange).toHaveBeenCalledWith("new@example.com");
    });

    await user.type(screen.getByLabelText("Verification code"), "123456");
    await user.click(screen.getByRole("button", { name: "Verify email" }));

    await waitFor(() => {
      expect(mockConfirmEmailChange).toHaveBeenCalledWith(
        "new@example.com",
        "123456",
      );
      expect(
        screen.queryByRole("heading", { name: "Change email address" }),
      ).not.toBeInTheDocument();
    });
  });

  it("does not mark a guest email as verified", () => {
    userRef.current = { ...userRef.current, is_guest: true };
    render(<AccountTab />, { wrapper: I18nWrapper });

    expect(screen.queryByText("Verified")).not.toBeInTheDocument();
  });

  it("auto-saves the complete controlled profile draft", async () => {
    const user = userEvent.setup();
    render(<AccountTab />, { wrapper: I18nWrapper });

    const firstName = screen.getByLabelText("First Name");
    await user.clear(firstName);
    await user.type(firstName, "Augusta");
    firstName.blur();

    await waitFor(() => {
      expect(mockUpdateMe).toHaveBeenCalledWith({
        timezone: "America/New_York",
        profile_description: "Builds compilers",
        profile_details: {
          first_name: "Augusta",
          last_name: "Lovelace",
          preferred_name: "Ada",
          username: "ada",
          role: "product-ops",
          phone: "+12065551243",
          website: "https://ada.example.com",
          start_week: "monday",
          time_format: "24-hour",
        },
      });
    });
    expect(mockSetUser).toHaveBeenCalled();
  });

  it("flushes the latest draft from the footer Save Changes action", async () => {
    const user = userEvent.setup();
    render(<AccountTab />, { wrapper: I18nWrapper });

    const firstName = screen.getByLabelText("First Name");
    await user.clear(firstName);
    await user.type(firstName, "Augusta");
    await user.click(screen.getByRole("button", { name: "Save Changes" }));

    await waitFor(() => {
      expect(mockUpdateMe).toHaveBeenCalledWith(
        expect.objectContaining({
          profile_details: expect.objectContaining({ first_name: "Augusta" }),
        }),
      );
    });
  });
});
