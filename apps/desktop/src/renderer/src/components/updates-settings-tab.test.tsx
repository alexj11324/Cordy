import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { LobeThemeBridge } from "@orvilo/views/lobe";

const mocks = vi.hoisted(() => ({
  getPreferences: vi.fn(),
  setAutomaticUpdates: vi.fn(),
  checkForUpdates: vi.fn(),
  toastSuccess: vi.fn(),
  toastError: vi.fn(),
}));

const translations = {
  auto_save: { toast_saved: "Settings saved" },
  desktop: {
    updates: {
      title: "Updates",
      section_title: "Version and updates",
      description: "Update preferences",
      current_version: "Current version",
      automatic_updates_title: "Automatic background updates",
      automatic_updates_description: "Download updates in the background",
      automatic_updates_save_failed: "Failed to save update settings",
      check_section_title: "Check for updates",
      check_section_description: "Check manually",
      up_to_date: "Up to date",
      downloading: "Downloading v{{version}}",
      check_now: "Check now",
      checking: "Checking",
    },
  },
};

vi.mock("@orvilo/views/i18n", () => ({
  useT: () => ({
    t: (
      selector: (resources: typeof translations) => string,
      values?: Record<string, string>,
    ) => {
      const template = selector(translations);
      return Object.entries(values ?? {}).reduce(
        (result, [key, value]) => result.replace(`{{${key}}}`, value),
        template,
      );
    },
  }),
}));

vi.mock("sonner", () => ({
  toast: {
    success: mocks.toastSuccess,
    error: mocks.toastError,
  },
}));

import { UpdatesSettingsTab } from "./updates-settings-tab";

/**
 * Mounting the Lobe tree is the expensive part of this suite, and it is the
 * *mount* rather than the module import: `LobeThemeBridge` brings up antd's
 * cssinjs runtime, which computes the whole component stylesheet on first
 * render. Isolated, the one test here finishes in about a second; inside a full
 * `vitest run` of the app it has been measured at **9.3s**, so the 5s default
 * fails a correct build. Widened rather than trimmed: the assertion polls for a
 * settled promise, so a generous budget costs nothing when it passes early, and
 * a failure here would be indistinguishable from a hang.
 */
const LOBE_MOUNT_TIMEOUT_MS = 20_000;
vi.setConfig({ testTimeout: LOBE_MOUNT_TIMEOUT_MS });

describe("UpdatesSettingsTab", () => {
  beforeEach(() => {
    mocks.getPreferences.mockReset().mockResolvedValue({
      automaticUpdates: true,
    });
    mocks.setAutomaticUpdates.mockReset();
    mocks.checkForUpdates.mockReset();
    mocks.toastSuccess.mockReset();
    mocks.toastError.mockReset();

    Object.defineProperty(window, "desktopAPI", {
      configurable: true,
      value: { appInfo: { version: "1.2.3" } },
    });
    Object.defineProperty(window, "updater", {
      configurable: true,
      value: {
        getPreferences: mocks.getPreferences,
        setAutomaticUpdates: mocks.setAutomaticUpdates,
        checkForUpdates: mocks.checkForUpdates,
      },
    });
  });

  it("loads the persisted preference and saves changes from the switch", async () => {
    mocks.getPreferences.mockResolvedValue({ automaticUpdates: false });
    mocks.setAutomaticUpdates.mockResolvedValue({ automaticUpdates: true });
    // Mounted inside `LobeThemeBridge` because the tab is on the Lobe shell
    // now: its group, its rows and its switch all call `useMotionComponent()`,
    // which throws without a `MotionProvider`. The bridge is the same one
    // `SettingsPage` mounts at runtime (`settings-page.tsx`, inside
    // `DialogContent`), so this suite renders the tree the app does.
    render(
      <LobeThemeBridge>
        <UpdatesSettingsTab />
      </LobeThemeBridge>,
    );

    const toggle = screen.getByRole("switch", {
      name: "Automatic background updates",
    });
    // The accessible name comes from `SettingsSwitch`'s required `label`, not
    // from the row: antd's `Form.Item` label is a `<label>` with no `for`,
    // because a field with no `name` never mints one.
    //
    // This used to read "the switch renders as a `<span role="switch">`, so
    // `toBeEnabled()` treats it as always enabled". That is no longer true —
    // the atom renders a real `<button role="switch" disabled>` — but the
    // assertion still waits on the persisted value rather than on the disabled
    // attribute, because the value is what this test is about.
    await waitFor(() => expect(toggle).not.toBeChecked());

    fireEvent.click(toggle);

    await waitFor(() => {
      expect(mocks.setAutomaticUpdates).toHaveBeenCalledWith(true);
      expect(toggle).toBeChecked();
    });
    expect(mocks.toastSuccess).toHaveBeenCalledWith("Settings saved", {
      id: "settings-auto-save",
    });
  });
});
