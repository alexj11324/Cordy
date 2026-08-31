// @vitest-environment jsdom
import type { ReactNode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, waitFor } from "@testing-library/react";
import { getActiveTab, useTabStore } from "@/stores/tab-store";

vi.mock("@patchbay/views/settings", () => ({
  SettingsPage: ({ navigationHeader }: { navigationHeader?: ReactNode }) => (
    <>{navigationHeader}</>
  ),
}));

type SettingsDictionary = {
  page: { title: string; back_to_app: string };
  desktop: { tabs: { updates: string } };
};

vi.mock("@patchbay/views/i18n", () => ({
  useT: () => ({
    t: (selector: (dictionary: SettingsDictionary) => string) =>
      selector({
        page: { title: "Settings", back_to_app: "Back to app" },
        desktop: { tabs: { updates: "Updates" } },
      }),
  }),
}));

vi.mock("./daemon-settings-tab", () => ({ DaemonSettingsTab: () => null }));
vi.mock("./updates-settings-tab", () => ({ UpdatesSettingsTab: () => null }));

import { DesktopSettingsPage } from "./desktop-settings-page";

beforeEach(() => {
  useTabStore.getState().reset();
  useTabStore.getState().switchWorkspace("acme");
  const active = getActiveTab(useTabStore.getState());
  if (active) useTabStore.getState().updateTab(active.id, { title: "Issues" });
  document.title = "Issues";
});

describe("DesktopSettingsPage window title", () => {
  it("owns the title while open and restores the latest active-tab title", async () => {
    const view = render(<DesktopSettingsPage onBack={() => {}} />);

    expect(document.title).toBe("Settings");

    document.title = "Late issue title";
    await waitFor(() => expect(document.title).toBe("Settings"));

    const active = getActiveTab(useTabStore.getState());
    if (active) {
      useTabStore.getState().updateTab(active.id, { title: "Updated issue" });
    }
    view.unmount();

    expect(document.title).toBe("Updated issue");
  });
});
