// @vitest-environment jsdom

import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, screen } from "@testing-library/react";

import { configStore } from "@orvilo/core/config";
import {
  BILLING_WORKSPACE_SUBSCRIPTIONS_FLAG,
  PLUGINS_V1_FLAG,
} from "@orvilo/core/feature-flags";
import { renderWithI18n } from "../../test/i18n";

const stub = vi.hoisted(
  () => (name: string) => () => ({ [name]: () => <div>{name}</div> }),
);
vi.mock("./account-tab", stub("AccountTab"));
vi.mock("./preferences-tab", stub("PreferencesTab"));
vi.mock("./issue-tab", stub("IssueTab"));
vi.mock("./tokens-tab", stub("TokensTab"));
vi.mock("./workspace-tab", stub("WorkspaceTab"));
vi.mock("./members-tab", stub("MembersTab"));
vi.mock("./repositories-tab", stub("RepositoriesTab"));
vi.mock("./github-tab", stub("GitHubTab"));
vi.mock("./integrations-tab", stub("IntegrationsTab"));
vi.mock("./labs-tab", stub("LabsTab"));
vi.mock("./notifications-tab", stub("NotificationsTab"));
vi.mock("./labels-tab", stub("LabelsTab"));
vi.mock("./properties-tab", stub("PropertiesTab"));
vi.mock("./quick-actions-tab", stub("QuickActionsTab"));
vi.mock("./keyboard-shortcuts-tab", stub("KeyboardShortcutsTab"));
vi.mock("./issue-statuses-tab", stub("IssueStatusesTab"));
vi.mock("./plugins-tab", stub("PluginsTab"));
vi.mock("./billing-tab", stub("BillingTab"));
vi.mock("./skills-tab", stub("SkillsTab"));
vi.mock("./mcp-tab", stub("McpTab"));

vi.mock("@orvilo/core/paths", () => ({
  useCurrentWorkspace: () => ({ name: "Acme" }),
  useWorkspacePaths: () => ({ issues: () => "/acme/issues" }),
}));

const replace = vi.fn();
const push = vi.fn();
const navigationState = { search: "" };
vi.mock("../../navigation", () => ({
  useNavigation: () => ({
    searchParams: new URLSearchParams(navigationState.search),
    hash: "",
    pathname: "/acme/settings",
    replace,
    push,
  }),
}));

const layout = { compact: true };
vi.mock("@orvilo/ui/hooks/use-mobile", () => ({
  useIsMobile: () => layout.compact,
  useIsCompact: () => layout.compact,
}));

import { SettingsPage } from "./settings-page";

beforeEach(() => {
  layout.compact = true;
  navigationState.search = "";
  configStore.getState().setFeatureFlags({});
  replace.mockClear();
  push.mockClear();
});

describe("SettingsPage flux dialog", () => {
  it("opens as a dialog with Cancel and Save changes", () => {
    renderWithI18n(<SettingsPage />);

    expect(screen.getByRole("dialog")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Cancel" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Save changes" })).toBeInTheDocument();
  });

  it("dismisses to issues when Cancel is pressed without an overlay handler", () => {
    renderWithI18n(<SettingsPage />);
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(push).toHaveBeenCalledWith("/acme/issues");
  });

  it("calls onDismiss instead of navigating when the overlay owns close", () => {
    const onDismiss = vi.fn();
    renderWithI18n(<SettingsPage onDismiss={onDismiss} />);
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(onDismiss).toHaveBeenCalled();
    expect(push).not.toHaveBeenCalled();
  });

  it("lets the tall left rail scroll so every existing section stays reachable", () => {
    layout.compact = false;
    renderWithI18n(<SettingsPage />);

    const profile = screen.getByRole("tab", { name: "Profile" });
    expect(profile.closest("aside")).toHaveClass("lg:overflow-y-auto");
  });

  it("keeps settings sections without duplicate member and floating-chat controls", () => {
    layout.compact = false;
    configStore.getState().setFeatureFlags({
      [PLUGINS_V1_FLAG]: true,
      [BILLING_WORKSPACE_SUBSCRIPTIONS_FLAG]: true,
    });

    renderWithI18n(<SettingsPage />);

    for (const name of [
      "Profile",
      "Preferences",
      "Shortcuts",
      "Issue",
      "Notifications",
      "API Tokens",
      "General",
      "Repositories",
      "GitHub",
      "Integrations",
      "Billing",
      "Labels",
      "Issue Statuses",
      "Properties",
      "Skills",
      "MCP",
      "Plugins",
    ]) {
      expect(screen.getByRole("tab", { name })).toBeInTheDocument();
    }
    expect(screen.queryByRole("tab", { name: "Labs" })).not.toBeInTheDocument();
    expect(screen.queryByRole("tab", { name: "Quick Actions" })).not.toBeInTheDocument();
  });

  it("opens Workspace General for the removed members URL", () => {
    navigationState.search = "tab=members";
    renderWithI18n(<SettingsPage />);
    expect(screen.queryByRole("tab", { name: "Members" })).not.toBeInTheDocument();
    expect(screen.getByText("WorkspaceTab")).toBeInTheDocument();
  });

  it("opens Preferences for the removed floating-chat settings URL", () => {
    navigationState.search = "tab=chat";
    renderWithI18n(<SettingsPage />);
    expect(screen.queryByRole("tab", { name: "Chat" })).not.toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "Preferences" })).toHaveAttribute("aria-selected", "true");
    expect(screen.getByText("PreferencesTab")).toBeInTheDocument();
  });

  it("marks standalone vs embedded on the dialog surface", () => {
    renderWithI18n(<SettingsPage variant="standalone" />);
    expect(
      document.querySelector("[data-settings-variant]"),
    ).toHaveAttribute("data-settings-variant", "standalone");
  });

  it("puts initial focus on Profile so the desktop overlay can land in the rail", () => {
    layout.compact = false;
    renderWithI18n(<SettingsPage />);
    expect(screen.getByRole("tab", { name: "Profile" })).toHaveAttribute(
      "data-settings-initial-focus",
    );
  });
});

describe("SettingsPage Plugin feature flag", () => {
  it("hides Plugins and falls back from a direct tab URL when disabled", () => {
    navigationState.search = "tab=plugins";

    renderWithI18n(<SettingsPage />);

    expect(screen.queryByRole("tab", { name: "Plugins" })).not.toBeInTheDocument();
    expect(screen.queryByText("PluginsTab")).not.toBeInTheDocument();
    expect(screen.getByText("AccountTab")).toBeInTheDocument();
  });

  it("shows and mounts Plugins when explicitly enabled", () => {
    navigationState.search = "tab=plugins";
    configStore.getState().setFeatureFlags({ [PLUGINS_V1_FLAG]: true });

    renderWithI18n(<SettingsPage />);

    expect(screen.getByRole("tab", { name: "Plugins" })).toBeInTheDocument();
    expect(screen.getByText("PluginsTab")).toBeInTheDocument();
  });
});

describe("SettingsPage workspace subscription feature flag", () => {
  it("hides Billing and falls back to Workspace General from a direct URL", () => {
    navigationState.search = "tab=billing";

    renderWithI18n(<SettingsPage />);

    expect(
      screen.queryByRole("tab", { name: "Billing" }),
    ).not.toBeInTheDocument();
    expect(screen.queryByText("BillingTab")).not.toBeInTheDocument();
    expect(screen.getByText("WorkspaceTab")).toBeInTheDocument();
  });

  it("shows and mounts Billing only when explicitly enabled", () => {
    navigationState.search = "tab=billing";
    configStore.getState().setFeatureFlags({
      [BILLING_WORKSPACE_SUBSCRIPTIONS_FLAG]: true,
    });

    renderWithI18n(<SettingsPage />);

    expect(screen.getByRole("tab", { name: "Billing" })).toBeInTheDocument();
    expect(screen.getByText("BillingTab")).toBeInTheDocument();
  });
});
