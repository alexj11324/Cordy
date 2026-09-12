import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const mockUpdateMe = vi.hoisted(() => vi.fn());
const mockRequestEmailChange = vi.hoisted(() => vi.fn());
const mockConfirmEmailChange = vi.hoisted(() => vi.fn());
const mockSetUser = vi.hoisted(() => vi.fn());
const mockToastSuccess = vi.hoisted(() => vi.fn());
const mockToastError = vi.hoisted(() => vi.fn());
const mockPersistLocale = vi.hoisted(() => vi.fn());
const mockReload = vi.hoisted(() => vi.fn());
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

vi.mock("@orvilo/core/i18n/react", async () => {
  const actual =
    await vi.importActual<typeof import("@orvilo/core/i18n/react")>(
      "@orvilo/core/i18n/react",
    );
  return {
    ...actual,
    useLocaleAdapter: () => ({
      persist: mockPersistLocale,
      getUserChoice: () => "en",
      getSystemPreferences: () => ["en"],
    }),
  };
});

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

import { renderWithI18n } from "../../test/i18n";
import { AccountTab } from "./account-tab";

// The whole suite mounts two Lobe `Form.Group`s and a modal's worth of
// motion-backed controls. Under full-suite parallelism a mount is seconds
// rather than milliseconds (the same reason `vitest.config.ts` raises the
// global budget for antd), so the hooks that unmount them get their own.
vi.setConfig({ testTimeout: 60_000, hookTimeout: 30_000 });

/**
 * `lobe: true` loads the theme bridge on demand, so the first query in every
 * test has to be an async one. This helper is that first query.
 *
 * It returns the two groups and every row query below goes through them: a
 * document-wide `getByRole(role, { name })` computes the accessible name of
 * every candidate in the document, which against the antd stylesheet the
 * bridge injects costs ~3.4s where the same query scoped to its own group
 * costs ~9ms.
 */
async function renderTab() {
  renderWithI18n(<AccountTab />, { lobe: true });
  const basic = within(
    await screen.findByRole("group", { name: "Basic Details" }),
  );
  const regional = within(
    await screen.findByRole("group", { name: "Regional Preferences" }),
  );
  return { basic, regional };
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

  it("renders the profile3 card with the real account fields", async () => {
    const { basic, regional } = await renderTab();

    expect(screen.getByTestId("profile-avatar")).toBeInTheDocument();
    expect(
      screen.queryByRole("heading", { name: "Profile" }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByText("Profile updates are shared"),
    ).not.toBeInTheDocument();
    expect(basic.getByLabelText(/First Name/)).toHaveValue("Ada");
    expect(basic.getByLabelText(/Last Name/)).toHaveValue("Lovelace");
    expect(basic.getByLabelText(/Primary Email Address/)).toHaveValue(
      "ada@example.com",
    );
    expect(basic.getByLabelText(/Preferred Name/)).toHaveValue("Ada");
    expect(basic.getByLabelText(/About You/)).toHaveValue("Builds compilers");
    expect(basic.getByLabelText(/Username/)).toHaveValue("ada");
    expect(basic.getByLabelText(/Phone Number/)).toHaveValue("+1 206 555 1243");
    expect(basic.getByLabelText(/Website/)).toHaveValue("ada.example.com");
    expect(
      basic.getByRole("combobox", { name: "Role" }),
    ).toBeInTheDocument();
    expect(
      regional.getByRole("combobox", { name: "Preferred Timezone" }),
    ).toBeInTheDocument();
    expect(
      regional.getByRole("combobox", { name: "Start Week On" }),
    ).toBeInTheDocument();
    expect(
      regional.getByRole("combobox", { name: "Language" }),
    ).toBeInTheDocument();
    expect(
      regional.getByRole("combobox", { name: "Time Format" }),
    ).toBeInTheDocument();
  });

  it("changes email through the request and verification flow", async () => {
    const user = userEvent.setup();
    const { basic } = await renderTab();

    await user.click(basic.getByRole("button", { name: "Edit" }));
    const email = await screen.findByLabelText("New email address");
    await user.clear(email);
    await user.type(email, "new@example.com");
    await user.click(screen.getByRole("button", { name: "Send code" }));

    await waitFor(() => {
      expect(mockRequestEmailChange).toHaveBeenCalledWith("new@example.com");
    });

    // The OTP is queried as a named `group`, not by label text: base-ui
    // resolves the `<label for>` to the field and then hangs its own
    // `aria-labelledby` on the root *and* on each of the six slots, so a label
    // query matches seven elements. The slots are the textboxes inside the
    // group — the field's eighth input is the `aria-hidden` value carrier
    // base-ui submits.
    const otp = await screen.findByRole("group", { name: "Verification code" });
    const firstSlot = within(otp).getAllByRole("textbox")[0]!;
    await user.click(firstSlot);
    await user.keyboard("123456");
    await user.click(screen.getByRole("button", { name: "Verify email" }));

    await waitFor(() => {
      expect(mockConfirmEmailChange).toHaveBeenCalledWith(
        "new@example.com",
        "123456",
      );
      expect(
        screen.queryByText("Change email address"),
      ).not.toBeInTheDocument();
    });
  });

  it("does not mark a guest email as verified", async () => {
    userRef.current = { ...userRef.current, is_guest: true };
    const { basic } = await renderTab();

    expect(basic.queryByText("Verified")).not.toBeInTheDocument();
  });

  it("auto-saves the complete controlled profile draft", async () => {
    const user = userEvent.setup();
    const { basic } = await renderTab();

    const firstName = basic.getByLabelText(/First Name/);
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
    const { basic } = await renderTab();

    const firstName = basic.getByLabelText(/First Name/);
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

  /**
   * The one control on this tab that is not a draft. `handleLanguageChange`
   * persists the locale cookie, PATCHes `/api/me` and reloads, so the assertion
   * that matters is that the PATCH happens on the *change* rather than through
   * `useAutoSave` 650ms later — and that no draft write goes with it.
   *
   * `window.location` is replaced because the change schedules a real
   * `reload()`, which jsdom cannot perform (this file and
   * `preferences-tab.test.tsx` stub it the same way, for the same reason).
   */
  describe("language", () => {
    beforeEach(() => {
      vi.useFakeTimers({ shouldAdvanceTime: true });
      Object.defineProperty(window, "location", {
        writable: true,
        configurable: true,
        value: { reload: mockReload },
      });
    });

    afterEach(() => {
      vi.useRealTimers();
    });

    it("writes the language change straight to the server rather than the draft", async () => {
      const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
      const { regional } = await renderTab();

      await user.click(regional.getByRole("combobox", { name: "Language" }));
      // The popup is reached through `[role=listbox]` — antd renders a clipped
      // mirror of the options for assistive technology, and the items a pointer
      // reaches carry no role. The click lands on the item next to the mirror.
      const mirror = await screen.findByRole("listbox");
      await user.click(
        within(mirror.parentElement as HTMLElement).getByTitle("简体中文"),
      );

      await waitFor(() => {
        expect(mockPersistLocale).toHaveBeenCalledWith("zh-Hans");
      });
      expect(mockUpdateMe).toHaveBeenCalledWith({ language: "zh-Hans" });
      expect(mockUpdateMe).toHaveBeenCalledTimes(1);
    });
  });
});
