// @vitest-environment jsdom
import type { ReactNode } from "react";
import { describe, expect, it, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import { getActiveTab, useTabStore } from "@/stores/tab-store";

const settingsPageProps = vi.hoisted(() => vi.fn());

vi.mock("@patchbay/views/settings", () => ({
  SettingsPage: (props: {
    navigationHeader?: ReactNode;
    variant?: "embedded" | "standalone";
    extraAccountTabs?: Array<{
      value: string;
      label: string;
      content: ReactNode;
    }>;
  }) => {
    settingsPageProps(props);
    return (
      <div data-testid="settings-page-mock">
        {props.navigationHeader}
        {props.extraAccountTabs?.map((tab) => (
          <div key={tab.value} data-testid={`settings-extra-${tab.value}`}>
            {tab.label}
            {tab.content}
          </div>
        ))}
      </div>
    );
  },
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

vi.mock("./daemon-settings-tab", () => ({
  DaemonSettingsTab: () => <div>Daemon settings panel</div>,
}));
vi.mock("./updates-settings-tab", () => ({
  UpdatesSettingsTab: () => <div>Updates settings panel</div>,
}));

import { DesktopSettingsPage } from "./desktop-settings-page";

beforeEach(() => {
  settingsPageProps.mockClear();
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

  it("injects the desktop-owned tabs and panels into standalone settings", () => {
    render(<DesktopSettingsPage onBack={() => {}} />);

    const props = settingsPageProps.mock.calls.at(-1)?.[0] as {
      variant?: string;
      extraAccountTabs?: Array<{
        value: string;
        label: string;
      }>;
    };
    expect(props.variant).toBe("standalone");
    expect(
      props.extraAccountTabs?.map(({ value, label }) => [value, label]),
    ).toEqual([
      ["daemon", "Daemon"],
      ["updates", "Updates"],
    ]);
    expect(screen.getByTestId("settings-extra-daemon")).toHaveTextContent(
      "Daemon settings panel",
    );
    expect(screen.getByTestId("settings-extra-updates")).toHaveTextContent(
      "Updates settings panel",
    );
  });

  it("renders a back button only when onBack is provided", () => {
    const { unmount } = render(<DesktopSettingsPage onBack={() => {}} />);
    expect(
      screen.getByRole("button", { name: "Back to app" }),
    ).toBeInTheDocument();
    unmount();

    render(<DesktopSettingsPage />);
    expect(
      screen.queryByRole("button", { name: "Back to app" }),
    ).not.toBeInTheDocument();
  });
});
