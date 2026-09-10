"use client";

import React from "react";
import {
  User,
  Search,
  X,
  SlidersHorizontal,
  Key,
  Settings,
  FolderGit2,
  Bell,
  Plug,
  Tags,
  CircleDot,
  Keyboard,
  ListTodo,
  Blocks,
  CreditCard,
  Server,
  Sparkles,
} from "lucide-react";
import { GitHubMark } from "./github-mark";
import { Tabs, TabsList, TabsTrigger, TabsContent } from "@orvilo/ui/components/ui/tabs";
import { Button } from "@orvilo/ui/components/ui/button";
import { Input } from "@orvilo/ui/components/ui/input";
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
import { useWorkspacePaths } from "@orvilo/core/paths";
import { useFeatureEnabled } from "@orvilo/core/config";
import {
  BILLING_WORKSPACE_SUBSCRIPTIONS_FLAG,
  PLUGINS_V1_FLAG,
} from "@orvilo/core/feature-flags";
import { useNavigation } from "../../navigation";
import { AccountTab } from "./account-tab";
import { PreferencesTab } from "./preferences-tab";
import { IssueTab } from "./issue-tab";
import { TokensTab } from "./tokens-tab";
import { WorkspaceTab } from "./workspace-tab";
import { RepositoriesTab } from "./repositories-tab";
import { GitHubTab } from "./github-tab";
import { IntegrationsTab } from "./integrations-tab";
import { NotificationsTab } from "./notifications-tab";
import { LabelsTab } from "./labels-tab";
import { IssueStatusesTab } from "./issue-statuses-tab";
import { PropertiesTab } from "./properties-tab";
import { KeyboardShortcutsTab } from "./keyboard-shortcuts-tab";
import { PluginsTab } from "./plugins-tab";
import { McpTab } from "./mcp-tab";
import { SkillsTab } from "./skills-tab";
import { BillingTab } from "./billing-tab";
import { SettingsDialogBody } from "./settings-layout";
import { useT } from "../../i18n";
import type { TFunction } from "i18next";

const ACCOUNT_TAB_KEYS = ["profile", "preferences", "shortcuts", "issue", "notifications", "tokens"] as const;
const ACCOUNT_TAB_ICONS = {
  profile: User,
  preferences: SlidersHorizontal,
  shortcuts: Keyboard,
  issue: ListTodo,
  notifications: Bell,
  tokens: Key,
} as const;

const WORKSPACE_TAB_KEYS = [
  "general",
  "repositories",
  "github",
  "integrations",
  "billing",
  "labels",
  "issue_statuses",
  "properties",
  "skills",
  "mcp",
  "plugins",
] as const;
const WORKSPACE_TAB_VALUES = {
  general: "workspace",
  repositories: "repositories",
  github: "github",
  integrations: "integrations",
  billing: "billing",
  labels: "labels",
  issue_statuses: "issue-statuses",
  properties: "properties",
  skills: "skills",
  mcp: "mcp",
  plugins: "plugins",
} as const;
const WORKSPACE_TAB_ICONS = {
  general: Settings,
  repositories: FolderGit2,
  github: GitHubMark,
  integrations: Plug,
  billing: CreditCard,
  labels: Tags,
  issue_statuses: CircleDot,
  properties: SlidersHorizontal,
  skills: Sparkles,
  mcp: Server,
  plugins: Blocks,
} as const;

const DEFAULT_TAB = "profile";
const TAB_QUERY_KEY = "tab";

const LEGACY_TAB_REDIRECTS: Record<string, string> = {
  lark: "integrations",
  members: "workspace",
  chat: "preferences",
};

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
  const [settingsSearch, setSettingsSearch] = React.useState("");
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
      : LEGACY_TAB_REDIRECTS[tabFromUrl] ?? tabFromUrl
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
            <div className="relative mb-3">
              <Search aria-hidden="true" className="pointer-events-none absolute top-1/2 left-2.5 size-3.5 -translate-y-1/2 text-muted-foreground" />
              <Input
                role="searchbox"
                value={settingsSearch}
                onChange={(event) => setSettingsSearch(event.target.value)}
                onKeyDown={(event) => {
                  if (event.key === "Escape" && settingsSearch) {
                    event.stopPropagation();
                    setSettingsSearch("");
                  }
                }}
                placeholder={t(($) => $.page.search_placeholder)}
                aria-label={t(($) => $.page.search_placeholder)}
                className="h-8 pr-8 pl-8"
              />
              {settingsSearch && (
                <button type="button" onClick={() => setSettingsSearch("")} aria-label={t(($) => $.page.search_clear)} className="absolute top-1/2 right-2 -translate-y-1/2 rounded p-0.5 text-muted-foreground hover:text-foreground">
                  <X aria-hidden="true" className="size-3.5" />
                </button>
              )}
            </div>
            <div className="min-w-0">
              {isMobile ? (
                <div className="-ml-5 overflow-x-auto overflow-y-hidden py-1 pl-5 [-ms-overflow-style:none] [scrollbar-width:none] [&::-webkit-scrollbar]:hidden">
                  <TabsList className="h-auto group-data-horizontal/tabs:h-auto w-max min-w-max justify-start gap-1 bg-transparent p-0">
                    <SettingsNavItems
                      activeTab={activeTab}
                      extraAccountTabs={extraAccountTabs}
                      visibleWorkspaceTabKeys={visibleWorkspaceTabKeys}
                      searchQuery={settingsSearch}
                    />
                  </TabsList>
                </div>
              ) : (
                <TabsList className="h-auto group-data-horizontal/tabs:h-auto w-full flex-col items-stretch gap-1 bg-transparent p-0">
                  <SettingsNavItems
                    activeTab={activeTab}
                    extraAccountTabs={extraAccountTabs}
                    visibleWorkspaceTabKeys={visibleWorkspaceTabKeys}
                    searchQuery={settingsSearch}
                  />
                </TabsList>
              )}
            </div>
          </aside>

          <div className="flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden">
            <DialogHeader className="shrink-0 gap-0 px-6 py-0 text-left">
              <div
                className="flex w-full flex-col gap-1.5 py-4 pr-12"
              >
                <DialogTitle className="text-title leading-6">
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
                className="flex w-full shrink-0 items-center justify-end gap-2 py-4"
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
  searchQuery,
}: {
  activeTab: string;
  extraAccountTabs?: ExtraSettingsTab[];
  visibleWorkspaceTabKeys: readonly (typeof WORKSPACE_TAB_KEYS)[number][];
  searchQuery: string;
}) {
  const { t } = useT("settings");
  const iconClassName = "size-4 shrink-0";
  const query = searchQuery.trim().toLocaleLowerCase();
  const matches = (value: string, label: string) =>
    `${value} ${label} ${tabDescription(value, t)}`.toLocaleLowerCase().includes(query);
  const accountTabs = ACCOUNT_TAB_KEYS.filter((key) => matches(key, t(($) => $.page.tabs[key])));
  const extraTabs = extraAccountTabs?.filter((tab) => matches(tab.value, tab.label)) ?? [];
  const workspaceTabs = visibleWorkspaceTabKeys.filter((key) => matches(WORKSPACE_TAB_VALUES[key], t(($) => $.page.tabs[key])));


  return (
    <>
      {accountTabs.map((key) => {
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
      {extraTabs.map((tab) => (
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
      {workspaceTabs.length > 0 && (
        <span className="hidden px-3 pt-3 pb-1 text-caption text-muted-foreground lg:block">
          {t(($) => $.page.workspace_settings)}
        </span>
      )}
      {accountTabs.length + extraTabs.length + workspaceTabs.length === 0 && (
        <p role="status" className="px-3 py-2 text-caption text-muted-foreground">{t(($) => $.page.search_empty)}</p>
      )}
      {workspaceTabs.map((key) => {
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
      <DialogTabPanel value="notifications"><NotificationsTab /></DialogTabPanel>
      <DialogTabPanel value="tokens"><TokensTab /></DialogTabPanel>
      <DialogTabPanel value="workspace"><WorkspaceTab /></DialogTabPanel>
      <DialogTabPanel value="repositories"><RepositoriesTab /></DialogTabPanel>
      <DialogTabPanel value="github"><GitHubTab /></DialogTabPanel>
      <DialogTabPanel value="integrations"><IntegrationsTab /></DialogTabPanel>
      {billingEnabled ? (
        <DialogTabPanel value="billing"><BillingTab /></DialogTabPanel>
      ) : null}
      <DialogTabPanel value="labels"><LabelsTab /></DialogTabPanel>
      <DialogTabPanel value="issue-statuses"><IssueStatusesTab /></DialogTabPanel>
      <DialogTabPanel value="properties"><PropertiesTab /></DialogTabPanel>
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
        <div className="min-w-0 px-6" data-slot="settings-tab-body">
          {children}
        </div>
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
    case "billing":
      return t(($) => $.page.tabs.billing);
    case "labels":
      return t(($) => $.page.tabs.labels);
    case "issue-statuses":
      return t(($) => $.page.tabs.issue_statuses);
    case "properties":
      return t(($) => $.page.tabs.properties);
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
