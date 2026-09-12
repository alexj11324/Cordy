"use client";

/**
 * The settings dialog shell: the rail, the header, the panels and the footer.
 *
 * Four things here are structural, and each one looks like a mistake until you
 * know why it is written that way:
 *
 * - **The rail is built from the base-ui Tabs *atoms*** (`TabsRoot` /
 *   `TabsList` / `TabsTab` / `TabsPanel`), not from a `Tabs` component. Both of
 *   Lobe's `Tabs` APIs are `items`-driven — `{ key, label, icon, children }` —
 *   so a `Tabs` renders its items and nothing else: there is no slot for the
 *   search box above the list, and none for the "workspace settings" group
 *   label or the empty state that sit *between* tabs. The atoms take arbitrary
 *   children and are built on `@base-ui/react/tabs`, so `role="tab"` and
 *   `aria-selected` are unchanged.
 *
 * - **`variant="point"` on the list and the tabs** selects the atoms'
 *   unopinionated base. The `rounded` default would put a segmented-control
 *   surface behind the whole rail and a pill behind every tab, neither of which
 *   is this rail's geometry.
 *
 * - **The theme bridge is mounted *inside* `DialogContent`.** Radix portals the
 *   dialog into `document.body`, and the bridge's antd tokens are CSS variables
 *   declared on a real element in its own subtree. A bridge wrapped *around*
 *   `DialogContent` would be an ancestor in the React tree only — context
 *   reaches through a portal, CSS variables do not — so every Lobe component
 *   inside the dialog would render with unset variables.
 *
 * - **The footer has one button, not Cancel + Save.** Every field in this
 *   dialog commits through its own control or through `useAutoSave`; there is
 *   no page-level save to flush (the migration reference's state-ownership rule
 *   says so outright: "There is no page-level save"). A button labelled "Save"
 *   that only closed the dialog was a lie about where the write happens, so
 *   the footer now says what the button does. It reads "Done" rather than
 *   "Close" because the dialog's own X is already named "Close" to a screen
 *   reader — two identically named buttons in one dialog is a riddle.
 */

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
  Keyboard,
  ListTodo,
  Blocks,
  CreditCard,
  Sparkles,
} from "lucide-react";
import {
  Button,
  Input,
  ScrollArea,
  TabsList,
  TabsPanel,
  TabsRoot,
  TabsTab,
} from "@lobehub/ui/base-ui";
import { GitHubMark } from "./github-mark";
import { McpMark } from "./mcp-mark";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@orvilo/ui/components/ui/dialog";
import { cn } from "@orvilo/ui/lib/utils";
import { useIsCompact } from "@orvilo/ui/hooks/use-mobile";
import { useWorkspacePaths } from "@orvilo/core/paths";
import { useFeatureEnabled } from "@orvilo/core/config";
import {
  BILLING_WORKSPACE_SUBSCRIPTIONS_FLAG,
  PLUGINS_V1_FLAG,
} from "@orvilo/core/feature-flags";
import { useNavigation } from "../../navigation";
import { LobeThemeBridge } from "../../lobe";
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
  skills: Sparkles,
  mcp: McpMark,
  plugins: Blocks,
} as const;

const DEFAULT_TAB = "profile";
const TAB_QUERY_KEY = "tab";

const LEGACY_TAB_REDIRECTS: Record<string, string> = {
  lark: "integrations",
  members: "workspace",
  chat: "preferences",
  "issue-statuses": "workspace",
  properties: "workspace",
};

/**
 * Overrides for Lobe's `variant="point"` tab base. The `!` marks are load
 * bearing: Lobe's tab/list styles come from antd-style, whose stylesheet order
 * against Tailwind's is not something this component can rely on.
 *
 * The rail has two geometries. Below `lg` it is a horizontal scroller whose
 * items must size to their content; at `lg` and up it is a full-width column
 * whose items stretch. `w-full` on a tab inside a `w-max` scroller is not
 * merely redundant — each tab would take the whole list's width and the
 * scroller would land on blank space past the label.
 */
const SETTINGS_NAV_LIST_BASE = "h-auto gap-1!";
const SETTINGS_NAV_LIST_CLASS = `${SETTINGS_NAV_LIST_BASE} w-full flex-col items-stretch`;
const SETTINGS_NAV_LIST_COMPACT_CLASS = `${SETTINGS_NAV_LIST_BASE} w-max min-w-max justify-start`;
const SETTINGS_NAV_TAB_BASE =
  "justify-start! gap-3! rounded-md px-3 py-1.5! data-[active]:text-brand!";
const SETTINGS_NAV_TAB_CLASS = `w-full ${SETTINGS_NAV_TAB_BASE}`;
const SETTINGS_NAV_TAB_COMPACT_CLASS = SETTINGS_NAV_TAB_BASE;

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
        {/* Renders no box of its own (`display: contents`), so `TabsRoot` is
            still the flex child `DialogContent` was laid out around. */}
        <LobeThemeBridge>
          <TabsRoot
            value={activeTab}
            onValueChange={handleTabChange}
            orientation={isMobile ? "horizontal" : "vertical"}
            className="flex min-h-0 flex-1 flex-col gap-0 lg:flex-row lg:items-stretch"
          >
            <aside className="bg-muted/20 flex shrink-0 flex-col border-b py-3 pr-12 pl-5 lg:min-h-0 lg:w-56 lg:self-stretch lg:overflow-y-auto lg:border-r lg:border-b-0 lg:py-4 lg:pr-5">
              <SettingsSearch
                value={settingsSearch}
                onValueChange={setSettingsSearch}
              />
              <div className="min-w-0">
                {isMobile ? (
                  <div className="-ml-5 overflow-x-auto overflow-y-hidden py-1 pl-5 [-ms-overflow-style:none] [scrollbar-width:none] [&::-webkit-scrollbar]:hidden">
                    <TabsList variant="point" className={SETTINGS_NAV_LIST_COMPACT_CLASS}>
                      <SettingsNavItems
                        activeTab={activeTab}
                        compact
                        extraAccountTabs={extraAccountTabs}
                        visibleWorkspaceTabKeys={visibleWorkspaceTabKeys}
                        searchQuery={settingsSearch}
                      />
                    </TabsList>
                  </div>
                ) : (
                  <TabsList variant="point" className={SETTINGS_NAV_LIST_CLASS}>
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
                <div className="flex w-full flex-col gap-1.5 py-4 pr-12">
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
                {/* `SettingsDialogBody` renders no DOM: it is a context that
                    tells a not-yet-migrated tab's `SettingsTab` to drop the
                    page heading the `DialogHeader` above already owns. It
                    cannot go until the last tab stops rendering `SettingsTab`
                    (tasks 4-9) — drop it earlier and every un-migrated tab
                    grows a second title and description above its body. */}
                <SettingsDialogBody>
                  <SettingsTabPanels
                    billingEnabled={billingEnabled}
                    extraAccountTabs={extraAccountTabs}
                    pluginsEnabled={pluginsEnabled}
                  />
                </SettingsDialogBody>
              </div>

              <DialogFooter className="m-0 border-0 bg-transparent! px-6 py-0">
                <div className="flex w-full shrink-0 items-center justify-end gap-2 py-4">
                  <Button onClick={dismiss}>{t(($) => $.page.dialog_done)}</Button>
                </div>
              </DialogFooter>
            </div>
          </TabsRoot>
        </LobeThemeBridge>
      </DialogContent>
    </Dialog>
  );
}

/**
 * The rail's search field. Built from Lobe's `Input` rather than `SearchBar`:
 * the rail needs the escape-to-clear key path and a *named* clear button
 * ("Clear search" is a translation), and `SearchBar`'s `allowClear` renders an
 * unnamed icon button that neither the tests nor a screen reader can address.
 */
function SettingsSearch({
  value,
  onValueChange,
}: {
  value: string;
  onValueChange: (next: string) => void;
}) {
  const { t } = useT("settings");
  const label = t(($) => $.page.search_placeholder);

  return (
    <Input
      role="searchbox"
      aria-label={label}
      placeholder={label}
      value={value}
      className="mb-3"
      prefix={<Search aria-hidden="true" className="size-3.5" />}
      suffix={
        value ? (
          <button
            type="button"
            onClick={() => onValueChange("")}
            aria-label={t(($) => $.page.search_clear)}
            className="text-muted-foreground hover:text-foreground rounded p-0.5"
          >
            <X aria-hidden="true" className="size-3.5" />
          </button>
        ) : null
      }
      onChange={(event) => onValueChange(event.target.value)}
      onKeyDown={(event) => {
        // The dialog closes on Escape; a search the user can see should clear
        // first, so this stops the event before it reaches the dialog.
        if (event.key === "Escape" && value) {
          event.stopPropagation();
          onValueChange("");
        }
      }}
    />
  );
}

function SettingsNavItems({
  activeTab,
  compact = false,
  extraAccountTabs,
  visibleWorkspaceTabKeys,
  searchQuery,
}: {
  activeTab: string;
  compact?: boolean;
  extraAccountTabs?: ExtraSettingsTab[];
  visibleWorkspaceTabKeys: readonly (typeof WORKSPACE_TAB_KEYS)[number][];
  searchQuery: string;
}) {
  const { t } = useT("settings");
  const tabClass = compact ? SETTINGS_NAV_TAB_COMPACT_CLASS : SETTINGS_NAV_TAB_CLASS;
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
          <TabsTab
            key={key}
            value={key}
            variant="point"
            data-settings-initial-focus={key === "profile" ? true : undefined}
            className={cn(tabClass, activeTab === key && "bg-muted!")}
          >
            <Icon className={iconClassName} aria-hidden="true" />
            <span className="truncate">{t(($) => $.page.tabs[key])}</span>
          </TabsTab>
        );
      })}
      {extraTabs.map((tab) => (
        <TabsTab
          key={tab.value}
          value={tab.value}
          variant="point"
          className={cn(tabClass, activeTab === tab.value && "bg-muted!")}
        >
          <tab.icon className={iconClassName} aria-hidden="true" />
          <span className="truncate">{tab.label}</span>
        </TabsTab>
      ))}
      {workspaceTabs.length > 0 && (
        <span className="text-caption text-muted-foreground hidden px-3 pt-3 pb-1 lg:block">
          {t(($) => $.page.workspace_settings)}
        </span>
      )}
      {accountTabs.length + extraTabs.length + workspaceTabs.length === 0 && (
        <p role="status" className="text-caption text-muted-foreground px-3 py-2">{t(($) => $.page.search_empty)}</p>
      )}
      {workspaceTabs.map((key) => {
        const Icon = WORKSPACE_TAB_ICONS[key];
        const value = WORKSPACE_TAB_VALUES[key];
        return (
          <TabsTab
            key={key}
            value={value}
            variant="point"
            className={cn(tabClass, activeTab === value && "bg-muted!")}
          >
            <Icon className={iconClassName} aria-hidden="true" />
            <span className="truncate">{t(($) => $.page.tabs[key])}</span>
          </TabsTab>
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
      <DialogTabPanel value="skills"><SkillsTab /></DialogTabPanel>
      <DialogTabPanel value="mcp"><McpTab /></DialogTabPanel>
      {pluginsEnabled ? <DialogTabPanel value="plugins"><PluginsTab /></DialogTabPanel> : null}
      {extraAccountTabs?.map((tab) => (
        <DialogTabPanel key={tab.value} value={tab.value}>{tab.content}</DialogTabPanel>
      ))}
    </>
  );
}

/**
 * One tab's scroll container.
 *
 * `pt-0!` cancels the top padding Lobe's panel adds — the DialogHeader above
 * already supplies that gap.
 *
 * `ScrollArea` is Lobe's, which is unstyled by design: base-ui's scroll area
 * expects the consumer to give the root its clipping and the viewport its
 * overflow. Lobe's own `content` element also sets a 12px font and a 16px gap
 * for its own composition, which would re-typeset every settings tab; the
 * inline style hands those back to the dialog's typography. Inline styles are
 * used rather than classes because antd-style's stylesheet order against
 * Tailwind's is not something this component can rely on.
 */
function DialogTabPanel({
  value,
  children,
}: {
  value: string;
  children: React.ReactNode;
}) {
  return (
    <TabsPanel
      value={value}
      className="mt-0 flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden pt-0!"
    >
      <ScrollArea
        className="min-h-0 flex-1 overflow-hidden"
        viewportProps={{ className: "h-full overflow-y-auto" }}
        contentProps={{
          style: { gap: 0, fontSize: "inherit", lineHeight: "inherit" },
        }}
      >
        <div className="min-w-0 px-6" data-slot="settings-tab-body">
          {children}
        </div>
      </ScrollArea>
    </TabsPanel>
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
