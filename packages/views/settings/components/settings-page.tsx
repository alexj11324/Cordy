"use client";

import React from "react";
import {
  User,
  SlidersHorizontal,
  Key,
  Settings,
  Users,
  FolderGit2,
  FlaskConical,
  Bell,
  Plug,
  MessageCircle,
  Tags,
  CircleDot,
  Keyboard,
  ListTodo,
  Zap,
  Blocks,
  CreditCard,
  Server,
} from "lucide-react";
import { GitHubMark } from "./github-mark";
import { Tabs, TabsList, TabsTrigger, TabsContent } from "@patchbay/ui/components/ui/tabs";
import { useIsMobile } from "@patchbay/ui/hooks/use-mobile";
import { useCurrentWorkspace } from "@patchbay/core/paths";
import { useFeatureEnabled } from "@patchbay/core/config";
import {
  BILLING_WORKSPACE_SUBSCRIPTIONS_FLAG,
  PLUGINS_V1_FLAG,
} from "@patchbay/core/feature-flags";
import { useNavigation } from "../../navigation";
import { AccountTab } from "./account-tab";
import { PreferencesTab } from "./preferences-tab";
import { ChatTab } from "./chat-tab";
import { IssueTab } from "./issue-tab";
import { TokensTab } from "./tokens-tab";
import { WorkspaceTab } from "./workspace-tab";
import { MembersTab } from "./members-tab";
import { RepositoriesTab } from "./repositories-tab";
import { GitHubTab } from "./github-tab";
import { IntegrationsTab } from "./integrations-tab";
import { LabsTab } from "./labs-tab";
import { NotificationsTab } from "./notifications-tab";
import { LabelsTab } from "./labels-tab";
import { IssueStatusesTab } from "./issue-statuses-tab";
import { PropertiesTab } from "./properties-tab";
import { QuickActionsTab } from "./quick-actions-tab";
import { KeyboardShortcutsTab } from "./keyboard-shortcuts-tab";
import { PluginsTab } from "./plugins-tab";
import { McpTab } from "./mcp-tab";
import { BillingTab } from "./billing-tab";
import { CollapsedNavTrigger } from "../../layout/page-header";
import { useT } from "../../i18n";
import { cn } from "@patchbay/ui/lib/utils";
import { LobeSettingsProvider } from "@patchbay/ui/components/common/lobe-settings-provider";
import { LobeSettingsTabs } from "@patchbay/ui/components/common/lobe-settings-tabs";

const ACCOUNT_TAB_KEYS = ["profile", "preferences", "shortcuts", "issue", "chat", "notifications", "tokens"] as const;
const ACCOUNT_TAB_ICONS = {
  profile: User,
  preferences: SlidersHorizontal,
  shortcuts: Keyboard,
  issue: ListTodo,
  chat: MessageCircle,
  notifications: Bell,
  tokens: Key,
} as const;

const WORKSPACE_TAB_KEYS = [
  "general",
  "repositories",
  "github",
  "integrations",
  "labs",
  "members",
  "billing",
  "labels",
  "issue_statuses",
  "properties",
  "quick_actions",
  "mcp",
  "plugins",
] as const;
const WORKSPACE_TAB_VALUES = {
  general: "workspace",
  repositories: "repositories",
  github: "github",
  integrations: "integrations",
  labs: "labs",
  members: "members",
  billing: "billing",
  labels: "labels",
  issue_statuses: "issue-statuses",
  properties: "properties",
  quick_actions: "quick-actions",
  mcp: "mcp",
  plugins: "plugins",
} as const;
const WORKSPACE_TAB_ICONS = {
  general: Settings,
  repositories: FolderGit2,
  github: GitHubMark,
  integrations: Plug,
  labs: FlaskConical,
  members: Users,
  billing: CreditCard,
  labels: Tags,
  issue_statuses: CircleDot,
  properties: SlidersHorizontal,
  quick_actions: Zap,
  mcp: Server,
  plugins: Blocks,
} as const;

const DEFAULT_TAB = "profile";
const TAB_QUERY_KEY = "tab";

// Legacy `?tab=…` values that have been collapsed into another tab. Old
// bookmarks still land on the correct surface without us preserving a
// dead TabsContent entry. Lark used to be its own top-level workspace
// tab; it now lives inside Integrations.
const LEGACY_WORKSPACE_TAB_REDIRECTS: Record<string, string> = {
  lark: "integrations",
};

const SETTINGS_TAB_TRIGGER_CLASS =
  "h-8 shrink-0 px-2.5 md:!w-full md:px-2 md:after:hidden";
const SETTINGS_EMBEDDED_TAB_TRIGGER_CLASS =
  "hover:bg-surface-hover data-active:!bg-surface-selected data-active:!text-surface-selected-foreground data-active:hover:!bg-surface-selected";
const SETTINGS_STANDALONE_TAB_TRIGGER_CLASS =
  "text-sidebar-text-secondary hover:bg-sidebar-item-hover hover:text-sidebar-item-active-foreground data-active:!bg-sidebar-item-active data-active:!text-sidebar-item-active-foreground data-active:hover:!bg-sidebar-item-active";

export interface ExtraSettingsTab {
  value: string;
  label: string;
  icon: React.ComponentType<{ className?: string }>;
  content: React.ReactNode;
}

export interface SettingsPageProps {
  /** Additional tabs injected by platform (e.g. desktop daemon settings) */
  extraAccountTabs?: ExtraSettingsTab[];
  /** Platform-owned control rendered above the settings navigation. */
  navigationHeader?: React.ReactNode;
  /** Use the app sidebar's visual language for a first-class settings surface. */
  variant?: "embedded" | "standalone";
}

export function SettingsPage({
  extraAccountTabs,
  navigationHeader,
  variant = "embedded",
}: SettingsPageProps = {}) {
  const { t } = useT("settings");
  const workspaceName = useCurrentWorkspace()?.name;
  const navigation = useNavigation();
  const isMobile = useIsMobile();
  const pluginsEnabled = useFeatureEnabled(PLUGINS_V1_FLAG, false);
  const billingEnabled = useFeatureEnabled(
    BILLING_WORKSPACE_SUBSCRIPTIONS_FLAG,
    false,
  );
  const isStandalone = variant === "standalone";
  const tabTriggerClass = cn(
    SETTINGS_TAB_TRIGGER_CLASS,
    isStandalone
      ? SETTINGS_STANDALONE_TAB_TRIGGER_CLASS
      : SETTINGS_EMBEDDED_TAB_TRIGGER_CLASS,
  );
  const visibleWorkspaceTabKeys = React.useMemo(
    () =>
      WORKSPACE_TAB_KEYS.filter(
        (key) =>
          (key !== "plugins" || pluginsEnabled) &&
          (key !== "billing" || billingEnabled),
      ),
    [billingEnabled, pluginsEnabled],
  );

  // Whitelist of valid tab values; unknown ?tab=… values silently fall back to
  // the default. Whitelisting also blocks junk like ?tab=<script> from
  // surfacing in the DOM via Radix Tabs internals.
  const validTabs = React.useMemo(
    () =>
      new Set<string>([
        ...ACCOUNT_TAB_KEYS,
        ...visibleWorkspaceTabKeys.map((key) => WORKSPACE_TAB_VALUES[key]),
        ...(extraAccountTabs?.map((tab) => tab.value) ?? []),
      ]),
    [extraAccountTabs, visibleWorkspaceTabKeys],
  );

  const tabFromUrl = navigation.searchParams.get(TAB_QUERY_KEY);
  const candidateTab = tabFromUrl
    ? tabFromUrl === "billing" && !billingEnabled
      ? "workspace"
      : LEGACY_WORKSPACE_TAB_REDIRECTS[tabFromUrl] ?? tabFromUrl
    : null;
  const activeTab =
    candidateTab && validTabs.has(candidateTab) ? candidateTab : DEFAULT_TAB;
  const hasWideContent =
    activeTab === "integrations" ||
    activeTab === "labels" ||
    activeTab === "issue-statuses" ||
    activeTab === "properties" ||
    activeTab === "quick-actions";
  const contentWidthClass = hasWideContent
    ? "max-w-5xl"
    : isStandalone
      ? "max-w-[57rem]"
      : "max-w-3xl";

  // replace (not push) so settings tab switches don't pollute browser history.
  // Preserve any other query params the page may carry.
  const handleTabChange = (next: string) => {
    const params = new URLSearchParams(navigation.searchParams);
    params.set(TAB_QUERY_KEY, next);
    navigation.replace(`${navigation.pathname}?${params.toString()}`);
  };

  const tabContents: Record<string, React.ReactNode> = {
    profile: <AccountTab />,
    preferences: <PreferencesTab />,
    shortcuts: <KeyboardShortcutsTab />,
    issue: <IssueTab />,
    chat: <ChatTab />,
    notifications: <NotificationsTab />,
    tokens: <TokensTab />,
    workspace: <WorkspaceTab />,
    repositories: <RepositoriesTab />,
    github: <GitHubTab />,
    integrations: <IntegrationsTab />,
    labs: <LabsTab />,
    members: <MembersTab />,
    billing: billingEnabled ? <BillingTab /> : null,
    labels: <LabelsTab />,
    "issue-statuses": <IssueStatusesTab />,
    properties: <PropertiesTab />,
    "quick-actions": <QuickActionsTab />,
    mcp: <McpTab />,
    plugins: pluginsEnabled ? <PluginsTab /> : null,
  };

  const accountTabItems = [
    ...ACCOUNT_TAB_KEYS.map((key) => {
      const Icon = ACCOUNT_TAB_ICONS[key];
      return {
        value: key,
        label: t(($) => $.page.tabs[key]),
        icon: <Icon className="h-4 w-4" />,
        content: tabContents[key],
      };
    }),
    ...(extraAccountTabs?.map((tab) => ({
      value: tab.value,
      label: tab.label,
      icon: <tab.icon className="h-4 w-4" />,
      content: tab.content,
    })) ?? []),
  ];
  const workspaceTabItems = visibleWorkspaceTabKeys.map((key) => {
    const Icon = WORKSPACE_TAB_ICONS[key];
    const value = WORKSPACE_TAB_VALUES[key];
    return {
      value,
      label: t(($) => $.page.tabs[key]),
      icon: <Icon className="h-4 w-4" />,
      content: tabContents[value],
    };
  });
  const lobeTabGroups = [
    {
      label: t(($) => $.page.my_account),
      tabs: accountTabItems,
    },
    {
      label: workspaceName ?? t(($) => $.page.workspace_fallback),
      tabs: workspaceTabItems,
    },
  ];

  const standaloneHeader = (
    <>
      {navigationHeader ? <div>{navigationHeader}</div> : null}
      {!navigationHeader ? (
        <div className="flex items-center md:mb-4">
          <CollapsedNavTrigger />
        </div>
      ) : null}
    </>
  );

  if (isStandalone) {
    return (
      <LobeSettingsProvider>
        <LobeSettingsTabs
          value={activeTab}
          onValueChange={handleTabChange}
          orientation={isMobile ? "horizontal" : "vertical"}
          groups={lobeTabGroups}
          header={standaloneHeader}
          dataSettingsVariant="standalone"
          className="flex min-h-0 flex-1 flex-col gap-0 overflow-y-auto bg-page-canvas text-foreground md:flex-row md:overflow-hidden"
          listClassName="flex w-max min-w-full flex-row items-center gap-1 overflow-x-auto border-b border-sidebar-border bg-sidebar p-2 md:w-64 md:min-w-0 md:flex-col md:items-stretch md:overflow-y-auto md:border-b-0 md:border-r md:p-4"
          contentClassName={cn(
            "mx-auto w-full p-4 sm:p-6 md:px-8",
            "md:pb-8 md:pt-20",
            contentWidthClass,
          )}
        />
      </LobeSettingsProvider>
    );
  }

  return (
    <Tabs
      value={activeTab}
      onValueChange={handleTabChange}
      orientation={isMobile ? "horizontal" : "vertical"}
      data-settings-variant={variant}
      className={cn(
        "flex min-h-0 flex-1 flex-col gap-0 overflow-y-auto md:flex-row md:overflow-hidden",
        isStandalone && "bg-page-canvas text-foreground",
      )}
    >
      <div
        className={cn(
          "shrink-0 overflow-x-auto border-b p-2 md:overflow-y-auto md:border-b-0 md:border-r md:p-4",
          isStandalone
            ? "border-sidebar-border bg-sidebar text-sidebar-text-primary md:w-80"
            : "border-surface-border md:w-56",
        )}
      >
        {navigationHeader ? (
          <div>{navigationHeader}</div>
        ) : null}
        {/* This page builds its own chrome instead of a PageHeader, so it has
            to supply the nav trigger itself — below `xl` the nav is a sheet or
            auto-collapsed, and settings has no other way back to it. */}
        {/* The gap below this row belongs to the row, not to the heading: with
            `items-center`, a bottom margin on the `h1` is part of the box being
            centred, so it offsets the heading against the trigger beside it. */}
        <div className="flex items-center md:mb-4">
          {navigationHeader ? null : <CollapsedNavTrigger />}
          <h1
            className={cn(
              "sr-only font-semibold md:not-sr-only md:px-2",
              isStandalone ? "text-title" : "text-body",
            )}
          >
            {t(($) => $.page.title)}
          </h1>
        </div>
        <TabsList
          variant="line"
          className="flex w-max min-w-full flex-row items-center gap-1 p-0 md:w-full md:flex-col md:items-stretch"
        >
          {/* My Account group */}
          <span className="hidden px-2 pb-1 pt-2 text-caption font-medium text-muted-foreground md:block">
            {t(($) => $.page.my_account)}
          </span>
          {ACCOUNT_TAB_KEYS.map((key) => {
            const Icon = ACCOUNT_TAB_ICONS[key];
            return (
              <TabsTrigger
                key={key}
                value={key}
                className={tabTriggerClass}
              >
                <Icon className="h-4 w-4" />
                {t(($) => $.page.tabs[key])}
              </TabsTrigger>
            );
          })}
          {extraAccountTabs?.map((tab) => (
            <TabsTrigger
              key={tab.value}
              value={tab.value}
              className={tabTriggerClass}
            >
              <tab.icon className="h-4 w-4" />
              {tab.label}
            </TabsTrigger>
          ))}

          {/* Workspace group */}
          <span className="hidden truncate px-2 pb-1 pt-4 text-caption font-medium text-muted-foreground md:block">
            {workspaceName ?? t(($) => $.page.workspace_fallback)}
          </span>
          {visibleWorkspaceTabKeys.map((key) => {
            const Icon = WORKSPACE_TAB_ICONS[key];
            return (
              <TabsTrigger
                key={key}
                value={WORKSPACE_TAB_VALUES[key]}
                className={tabTriggerClass}
              >
                <Icon className="h-4 w-4" />
                {t(($) => $.page.tabs[key])}
              </TabsTrigger>
            );
          })}
        </TabsList>
      </div>

      {/* Right content */}
      <div
        className={cn(
          "min-w-0 flex-1 md:overflow-y-auto",
          isStandalone && "bg-page-canvas",
        )}
      >
        <div
          data-settings-content
          className={cn(
            "mx-auto w-full p-4 sm:p-6 md:px-8",
            isStandalone ? "md:pb-8 md:pt-20" : "md:py-7",
            contentWidthClass,
          )}
        >
          {Object.entries(tabContents).map(([value, content]) =>
            content ? (
              <TabsContent key={value} value={value}>
                {content}
              </TabsContent>
            ) : null,
          )}
          {extraAccountTabs?.map((tab) => (
            <TabsContent key={tab.value} value={tab.value}>{tab.content}</TabsContent>
          ))}
        </div>
      </div>
    </Tabs>
  );
}
