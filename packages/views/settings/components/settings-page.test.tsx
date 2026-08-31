import { cleanup, fireEvent, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { SidebarProvider, useSidebar } from "@patchbay/ui/components/ui/sidebar";
import { configStore } from "@patchbay/core/config";
import {
  BILLING_WORKSPACE_SUBSCRIPTIONS_FLAG,
  PLUGINS_V1_FLAG,
} from "@patchbay/core/feature-flags";
import { renderWithI18n } from "../../test/i18n";

// This file tests the settings SHELL — the chrome around the tabs — so every
// tab panel is stubbed out. Their contents have their own test files.
const stub = vi.hoisted(
  () => (name: string) => () => ({ [name]: () => <div>{name}</div> }),
);
vi.mock("./account-tab", stub("AccountTab"));
vi.mock("./preferences-tab", stub("PreferencesTab"));
vi.mock("./chat-tab", stub("ChatTab"));
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
vi.mock("./issue-statuses-tab", stub("IssueStatusesTab"));
vi.mock("./properties-tab", stub("PropertiesTab"));
vi.mock("./quick-actions-tab", stub("QuickActionsTab"));
vi.mock("./keyboard-shortcuts-tab", stub("KeyboardShortcutsTab"));
vi.mock("./mcp-tab", stub("McpTab"));
vi.mock("./plugins-tab", stub("PluginsTab"));
vi.mock("./billing-tab", stub("BillingTab"));

vi.mock("@patchbay/core/paths", () => ({
  useCurrentWorkspace: () => ({ name: "Acme" }),
}));

const replace = vi.fn();
const navigationState = { search: "" };
vi.mock("../../navigation", () => ({
  useNavigation: () => ({
    searchParams: new URLSearchParams(navigationState.search),
    pathname: "/acme/settings",
    replace,
  }),
}));

// Compact by default: that is the width where the nav is a sheet and this
// trigger is the only way to reach it.
const layout = { compact: true };
vi.mock("@patchbay/ui/hooks/use-mobile", () => ({
  useIsMobile: () => layout.compact,
  useIsCompact: () => layout.compact,
}));

import { SettingsPage } from "./settings-page";

function NavStateProbe() {
  const { openMobile } = useSidebar();
  return <div data-testid="nav-open">{String(openMobile)}</div>;
}

function trigger() {
  return screen.getByRole("button", { name: "Toggle Sidebar" });
}

beforeEach(() => {
  layout.compact = true;
  navigationState.search = "";
  configStore.getState().setFeatureFlags({});
  replace.mockClear();
});

describe("SettingsPage nav trigger", () => {
  it("opens the nav from settings at compact widths", () => {
    // Settings builds its own chrome instead of a PageHeader, so without this
    // control a touch user who lands here has no way back to the nav at all —
    // the keyboard shortcut is not an answer on a tablet.
    renderWithI18n(
      <SidebarProvider>
        <NavStateProbe />
        <SettingsPage />
      </SidebarProvider>,
    );

    expect(screen.getByTestId("nav-open").textContent).toBe("false");

    fireEvent.click(trigger());

    expect(screen.getByTestId("nav-open").textContent).toBe("true");
  });

  it("hides the trigger only where the nav is a permanent column", () => {
    // The nav is in-flow from `xl` up, so the control is CSS-gated rather than
    // unmounted — jsdom applies no stylesheet, hence the class assertion.
    renderWithI18n(
      <SidebarProvider>
        <SettingsPage />
      </SidebarProvider>,
    );

    expect(trigger().className).toContain("xl:hidden");
  });

  it("still renders standalone, without a sidebar around it", () => {
    // Desktop mounts settings inside its own shell; the trigger has to no-op
    // rather than throw when there is no SidebarProvider above it.
    renderWithI18n(<SettingsPage />);

    expect(
      screen.queryByRole("button", { name: "Toggle Sidebar" }),
    ).not.toBeInTheDocument();
    expect(screen.getByText("Settings")).toBeInTheDocument();
  });

  it("uses a platform navigation header instead of the app-sidebar trigger", () => {
    renderWithI18n(
      <SettingsPage
        navigationHeader={<button type="button">Back to app</button>}
      />,
    );

    expect(
      screen.getByRole("button", { name: "Back to app" }),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "Toggle Sidebar" }),
    ).not.toBeInTheDocument();
  });

  it("uses the shared sidebar language for a standalone settings surface", async () => {
    const { container } = renderWithI18n(
      <SettingsPage variant="standalone" />,
    );

    await waitFor(() => {
      expect(
        container.querySelector('[data-settings-ui="lobe-runtime"]'),
      ).toBeInTheDocument();
    });
    const settings = container.querySelector('[data-settings-ui="lobe"]');
    const navigation = container.querySelector(
      '[data-settings-ui="lobe"] [role="tablist"]',
    );
    expect(settings).toHaveClass("bg-page-canvas");
    expect(container.querySelector('[data-settings-ui="lobe"]')).toBeInTheDocument();
    expect(navigation).toHaveAttribute("data-settings-sidebar-glass", "true");
    expect(navigation).toHaveClass("md:w-64");
    expect(navigation).not.toHaveClass("md:w-80");
    expect(navigation).not.toHaveClass("md:w-56");
    expect(container.querySelector("[data-settings-content]")).toHaveClass(
      "max-w-[57rem]",
    );
    expect(screen.getByRole("tab", { name: "Profile" })).toHaveClass(
      "data-active:!bg-sidebar-item-active",
    );
  });

  it("does not repeat the settings title in the standalone navigation", async () => {
    const { container } = renderWithI18n(<SettingsPage variant="standalone" />);

    await waitFor(() => {
      expect(
        container.querySelector('[data-settings-ui="lobe-runtime"]'),
      ).toBeInTheDocument();
    });
    expect(
      screen.queryByRole("heading", { name: "Settings" }),
    ).not.toBeInTheDocument();
  });

  it("keeps every existing settings destination visible in the standalone nav", async () => {
    renderWithI18n(
      <SettingsPage
        variant="standalone"
        extraAccountTabs={[
          { value: "daemon", label: "Daemon", icon: () => null, content: null },
          { value: "updates", label: "Updates", icon: () => null, content: null },
        ]}
      />,
    );

    await waitFor(() => {
      expect(
        screen.getAllByRole("tab").map((tab) => tab.textContent?.trim()),
      ).toEqual([
        "Profile",
        "Preferences",
        "Shortcuts",
        "Issue",
        "Chat",
        "Notifications",
        "API Tokens",
        "Daemon",
        "Updates",
        "General",
        "Repositories",
        "GitHub",
        "Integrations",
        "Labs",
        "Members",
        "Labels",
        "Issue Statuses",
        "Properties",
        "Quick Actions",
        "MCP",
      ]);
    });
  });

  it("keeps every standalone destination wired to its existing panel", async () => {
    const panels = [
      ["profile", "AccountTab"],
      ["preferences", "PreferencesTab"],
      ["shortcuts", "KeyboardShortcutsTab"],
      ["issue", "IssueTab"],
      ["chat", "ChatTab"],
      ["notifications", "NotificationsTab"],
      ["tokens", "TokensTab"],
      ["daemon", "DaemonPanel"],
      ["updates", "UpdatesPanel"],
      ["workspace", "WorkspaceTab"],
      ["repositories", "RepositoriesTab"],
      ["github", "GitHubTab"],
      ["integrations", "IntegrationsTab"],
      ["labs", "LabsTab"],
      ["members", "MembersTab"],
      ["labels", "LabelsTab"],
      ["issue-statuses", "IssueStatusesTab"],
      ["properties", "PropertiesTab"],
      ["quick-actions", "QuickActionsTab"],
      ["mcp", "McpTab"],
    ] as const;

    for (const [value, panel] of panels) {
      cleanup();
      navigationState.search = `tab=${value}`;
      renderWithI18n(
        <SettingsPage
          variant="standalone"
          extraAccountTabs={[
            {
              value: "daemon",
              label: "Daemon",
              icon: () => null,
              content: <div>DaemonPanel</div>,
            },
            {
              value: "updates",
              label: "Updates",
              icon: () => null,
              content: <div>UpdatesPanel</div>,
            },
          ]}
        />,
      );

      await waitFor(() => {
        expect(screen.getByText(panel)).toBeInTheDocument();
      });
    }
  }, 30_000);

  it("preserves the embedded settings navigation width", () => {
    const { container } = renderWithI18n(<SettingsPage />);
    const navigation = container.querySelector(
      '[data-settings-variant="embedded"] > div',
    );

    expect(navigation).toHaveClass("md:w-56");
    expect(navigation).not.toHaveClass("md:w-80");
    expect(container.querySelector("[data-settings-content]")).toHaveClass(
      "max-w-3xl",
    );
    expect(
      container.querySelector("[data-settings-content]"),
    ).not.toHaveClass("max-w-[57rem]");
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
