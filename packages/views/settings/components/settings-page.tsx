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
  Sparkles,
} from "lucide-react";
import { GitHubMark } from "./github-mark";
import { Tabs, TabsList, TabsTrigger, TabsContent } from "@orvilo/ui/components/ui/tabs";
import { Button } from "@orvilo/ui/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@orvilo/ui/components/ui/dialog";
import { ScrollArea } from "@orvilo/ui/components/ui/scroll-area";
import { cn } from "@orvilo/ui/lib/utils";
import { useIsCompact } from "@orvilo/ui/hooks/use-mobile";
import { useCurrentWorkspace, useWorkspacePaths } from "@orvilo/core/paths";
import { useFeatureEnabled } from "@orvilo/core/config";
import {
  BILLING_WORKSPACE_SUBSCRIPTIONS_FLAG,
  PLUGINS_V1_FLAG,
} from "@orvilo/core/feature-flags";
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
import { SkillsTab } from "./skills-tab";
import { BillingTab } from "./billing-tab";
import { SettingsDialogBody } from "./settings-layout";
import { useT } from "../../i18n";
import type { TFunction } from "i18next";

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
  "skills",
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
  skills: "skills",
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
  skills: Sparkles,
  mcp: Server,
  plugins: Blocks,
} as const;

const DEFAULT_TAB = "profile";
const TAB_QUERY_KEY = "tab";

const LEGACY_WORKSPACE_TAB_REDIRECTS: Record<string, string> = {
  lark: "integrations",
};

const DIALOG_INSET_SEPARATOR_CLASS = "border-border w-full";
const SETTINGS_NAV_TRIGGER_CLASS =
  "w-full justify-start gap-3 px-3 py-1.5 shadow-none";

export interface ExtraSettingsTab {
  value: string;
  label: string;
  icon: React.ComponentType<{ className?: string }>;
  content: React.ReactNode;
}

interface SettingsPageProps {
  extraAccountTabs?: ExtraSettingsTab[];
  variant?: "embedded" | "standalone";
  /** @deprecated The flux dialog uses Cancel / Save instead of a back row. */
  navigationHeader?: React.ReactNode;
  onDismiss?: () => void;
}

export function SettingsPage({
  extraAccountTabs,
  variant = "embedded",
  onDismiss,
}: SettingsPageProps = {}) {
  const { t } = useT("settings");
  const workspaceName = useCurrentWorkspace()?.name;
  const navigation = useNavigation();
  const paths = useWorkspacePaths();
  const isMobile = useIsCompact();
  const pluginsEnabled = useFeatureEnabled(PLUGINS_V1_FLAG, false);
  const billingEnabled = useFeatureEnabled(
    BILLING_WORKSPACE_SUBSCRIPTIONS_FLAG,
    false,
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

  const handleTabChange = (next: string) => {
    const params = new URLSearchParams(navigation.searchParams);
    params.set(TAB_QUERY_KEY, next);
    navigation.replace(`${navigation.pathname}?${params.toString()}`);
  };

  const dismiss = () => {
    if (onDismiss) {
      onDismiss();
      return;
    }
    navigation.push(paths.issues());
  };

  const activeTitle = tabTitle(activeTab, t, extraAccountTabs);
  const activeDescription = tabDescription(activeTab, t);

  return (
    <Dialog
      open
      onOpenChange={(next) => {
        if (!next) dismiss();
      }}
    >
      <DialogContent
        data-settings-variant={variant}
        className="flex h-[min(88svh,48rem)] max-w-[calc(100vw-1.5rem)] flex-col gap-0 overflow-hidden p-0 **:data-[slot=dialog-close]:top-4! **:data-[slot=dialog-close]:right-4! sm:max-w-4xl lg:max-w-[60rem]"
      >
        <Tabs
          value={activeTab}
          onValueChange={handleTabChange}
          orientation={isMobile ? "horizontal" : "vertical"}
          className="min-h-0 flex-1 flex-col gap-0 lg:flex-row lg:items-stretch"
        >
          <aside className="bg-muted/20 flex shrink-0 flex-col border-b py-3 pr-12 pl-5 lg:min-h-0 lg:w-56 lg:self-stretch lg:overflow-y-auto lg:border-r lg:border-b-0 lg:py-4 lg:pr-5">
            <div className="min-w-0">
              {isMobile ? (
                <div className="-ml-5 overflow-x-auto overflow-y-hidden py-1 pl-5 [-ms-overflow-style:none] [scrollbar-width:none] [&::-webkit-scrollbar]:hidden">
                  <TabsList className="h-auto group-data-horizontal/tabs:h-auto w-max min-w-max justify-start gap-1 bg-transparent p-0">
                    <SettingsNavItems
                      activeTab={activeTab}
                      extraAccountTabs={extraAccountTabs}
                      visibleWorkspaceTabKeys={visibleWorkspaceTabKeys}
                      workspaceName={workspaceName}
                    />
                  </TabsList>
                </div>
              ) : (
                <TabsList className="h-auto group-data-horizontal/tabs:h-auto w-full flex-col items-stretch gap-1 bg-transparent p-0">
                  <SettingsNavItems
                    activeTab={activeTab}
                    extraAccountTabs={extraAccountTabs}
                    visibleWorkspaceTabKeys={visibleWorkspaceTabKeys}
                    workspaceName={workspaceName}
                  />
                </TabsList>
              )}
            </div>
          </aside>

          <div className="flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden">
            <DialogHeader className="shrink-0 gap-0 px-6 py-0 text-left">
              <div
                className={cn(
                  DIALOG_INSET_SEPARATOR_CLASS,
                  "flex flex-col gap-1.5 border-b py-4 pr-12",
                )}
              >
                <DialogTitle className="text-lg leading-6">
                  {activeTitle}
                </DialogTitle>
                {activeDescription ? (
                  <DialogDescription>{activeDescription}</DialogDescription>
                ) : (
                  <DialogDescription className="sr-only">
                    {activeTitle}
                  </DialogDescription>
                )}
              </div>
            </DialogHeader>

            <div
              className="flex min-h-0 flex-1 flex-col overflow-hidden"
              data-slot="settings-content-surface"
            >
              <SettingsDialogBody>
                <SettingsTabPanels
                  billingEnabled={billingEnabled}
                  extraAccountTabs={extraAccountTabs}
                  pluginsEnabled={pluginsEnabled}
                />
              </SettingsDialogBody>
            </div>

            <DialogFooter className="m-0 border-0 bg-transparent! px-6 py-0">
              <div
                className={cn(
                  DIALOG_INSET_SEPARATOR_CLASS,
                  "flex shrink-0 items-center justify-end gap-2 border-t py-4",
                )}
              >
                <Button type="button" variant="outline" onClick={dismiss}>
                  {t(($) => $.page.dialog_cancel)}
                </Button>
                <Button type="button" onClick={dismiss}>
                  {t(($) => $.page.dialog_save)}
                </Button>
              </div>
            </DialogFooter>
          </div>
        </Tabs>
      </DialogContent>
    </Dialog>
  );
}

function SettingsNavItems({
  activeTab,
  extraAccountTabs,
  visibleWorkspaceTabKeys,
  workspaceName,
}: {
  activeTab: string;
  extraAccountTabs?: ExtraSettingsTab[];
  visibleWorkspaceTabKeys: readonly (typeof WORKSPACE_TAB_KEYS)[number][];
  workspaceName?: string;
}) {
  const { t } = useT("settings");
  const iconClassName = "size-4 shrink-0";

  return (
    <>
      {ACCOUNT_TAB_KEYS.map((key) => {
        const Icon = ACCOUNT_TAB_ICONS[key];
        return (
          <TabsTrigger
            key={key}
            value={key}
            data-settings-initial-focus={key === "profile" ? true : undefined}
            className={cn(
              SETTINGS_NAV_TRIGGER_CLASS,
              activeTab === key ? "bg-muted!" : "bg-transparent",
            )}
          >
            <Icon className={iconClassName} aria-hidden="true" />
            <span className="truncate">{t(($) => $.page.tabs[key])}</span>
          </TabsTrigger>
        );
      })}
      {extraAccountTabs?.map((tab) => (
        <TabsTrigger
          key={tab.value}
          value={tab.value}
          className={cn(
            SETTINGS_NAV_TRIGGER_CLASS,
            activeTab === tab.value ? "bg-muted!" : "bg-transparent",
          )}
        >
          <tab.icon className={iconClassName} aria-hidden="true" />
          <span className="truncate">{tab.label}</span>
        </TabsTrigger>
      ))}
      <span className="hidden px-3 pt-3 pb-1 text-caption text-muted-foreground lg:block">
        {workspaceName ?? t(($) => $.page.workspace_fallback)}
      </span>
      {visibleWorkspaceTabKeys.map((key) => {
        const Icon = WORKSPACE_TAB_ICONS[key];
        const value = WORKSPACE_TAB_VALUES[key];
        return (
          <TabsTrigger
            key={key}
            value={value}
            className={cn(
              SETTINGS_NAV_TRIGGER_CLASS,
              activeTab === value ? "bg-muted!" : "bg-transparent",
            )}
          >
            <Icon className={iconClassName} aria-hidden="true" />
            <span className="truncate">{t(($) => $.page.tabs[key])}</span>
          </TabsTrigger>
        );
      })}
    </>
  );
}

function SettingsTabPanels({
  billingEnabled,
  extraAccountTabs,
  pluginsEnabled,
}: {
  billingEnabled: boolean;
  extraAccountTabs?: ExtraSettingsTab[];
  pluginsEnabled: boolean;
}) {
  return (
    <>
      <DialogTabPanel value="profile"><AccountTab /></DialogTabPanel>
      <DialogTabPanel value="preferences"><PreferencesTab /></DialogTabPanel>
      <DialogTabPanel value="shortcuts"><KeyboardShortcutsTab /></DialogTabPanel>
      <DialogTabPanel value="issue"><IssueTab /></DialogTabPanel>
      <DialogTabPanel value="chat"><ChatTab /></DialogTabPanel>
      <DialogTabPanel value="notifications"><NotificationsTab /></DialogTabPanel>
      <DialogTabPanel value="tokens"><TokensTab /></DialogTabPanel>
      <DialogTabPanel value="workspace"><WorkspaceTab /></DialogTabPanel>
      <DialogTabPanel value="repositories"><RepositoriesTab /></DialogTabPanel>
      <DialogTabPanel value="github"><GitHubTab /></DialogTabPanel>
      <DialogTabPanel value="integrations"><IntegrationsTab /></DialogTabPanel>
      <DialogTabPanel value="labs"><LabsTab /></DialogTabPanel>
      <DialogTabPanel value="members"><MembersTab /></DialogTabPanel>
      {billingEnabled ? (
        <DialogTabPanel value="billing"><BillingTab /></DialogTabPanel>
      ) : null}
      <DialogTabPanel value="labels"><LabelsTab /></DialogTabPanel>
      <DialogTabPanel value="issue-statuses"><IssueStatusesTab /></DialogTabPanel>
      <DialogTabPanel value="properties"><PropertiesTab /></DialogTabPanel>
      <DialogTabPanel value="quick-actions"><QuickActionsTab /></DialogTabPanel>
      <DialogTabPanel value="skills"><SkillsTab /></DialogTabPanel>
      <DialogTabPanel value="mcp"><McpTab /></DialogTabPanel>
      {pluginsEnabled ? <DialogTabPanel value="plugins"><PluginsTab /></DialogTabPanel> : null}
      {extraAccountTabs?.map((tab) => (
        <DialogTabPanel key={tab.value} value={tab.value}>{tab.content}</DialogTabPanel>
      ))}
    </>
  );
}

function DialogTabPanel({
  value,
  children,
}: {
  value: string;
  children: React.ReactNode;
}) {
  return (
    <TabsContent
      value={value}
      className="mt-0 flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden"
    >
      <ScrollArea
        className={cn(
          "min-h-0 flex-1",
          "**:data-[slot=scroll-area-scrollbar]:opacity-0 **:data-[slot=scroll-area-scrollbar]:transition-opacity **:data-[slot=scroll-area-scrollbar]:duration-150 hover:**:data-[slot=scroll-area-scrollbar]:opacity-100",
        )}
      >
        {children}
      </ScrollArea>
    </TabsContent>
  );
}

function tabTitle(
  tab: string,
  t: TFunction<"settings">,
  extraAccountTabs?: ExtraSettingsTab[],
): string {
  const extra = extraAccountTabs?.find((candidate) => candidate.value === tab);
  if (extra) return extra.label;
  switch (tab) {
    case "profile":
      return t(($) => $.page.tabs.profile);
    case "preferences":
      return t(($) => $.page.tabs.preferences);
    case "shortcuts":
      return t(($) => $.page.tabs.shortcuts);
    case "issue":
      return t(($) => $.page.tabs.issue);
    case "chat":
      return t(($) => $.page.tabs.chat);
    case "notifications":
      return t(($) => $.page.tabs.notifications);
    case "tokens":
      return t(($) => $.page.tabs.tokens);
    case "workspace":
      return t(($) => $.page.tabs.general);
    case "repositories":
      return t(($) => $.page.tabs.repositories);
    case "github":
      return t(($) => $.page.tabs.github);
    case "integrations":
      return t(($) => $.page.tabs.integrations);
    case "labs":
      return t(($) => $.page.tabs.labs);
    case "members":
      return t(($) => $.page.tabs.members);
    case "billing":
      return t(($) => $.page.tabs.billing);
    case "labels":
      return t(($) => $.page.tabs.labels);
    case "issue-statuses":
      return t(($) => $.page.tabs.issue_statuses);
    case "properties":
      return t(($) => $.page.tabs.properties);
    case "quick-actions":
      return t(($) => $.page.tabs.quick_actions);
    case "skills":
      return t(($) => $.page.tabs.skills);
    case "mcp":
      return t(($) => $.page.tabs.mcp);
    case "plugins":
      return t(($) => $.page.tabs.plugins);
    default:
      return t(($) => $.page.title);
  }
}

function tabDescription(
  tab: string,
  t: TFunction<"settings">,
): string {
  switch (tab) {
    case "profile":
      return t(($) => $.account.page_description);
    case "preferences":
      return t(($) => $.preferences.page_description);
    case "shortcuts":
      return t(($) => $.shortcuts.description);
    case "notifications":
      return t(($) => $.notifications.page_description);
    case "workspace":
      return t(($) => $.workspace.page_description);
    case "github":
      return t(($) => $.github.page_description);
    case "integrations":
      return t(($) => $.page.integrations_description);
    default:
      return "";
  }
}
