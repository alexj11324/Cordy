"use client";

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import type {
  Agent,
  AgentRuntime,
  MemberWithUser,
} from "@patchbay/core/types";
import { useWorkspaceId } from "@patchbay/core/hooks";
import { larkInstallationsOptions } from "@patchbay/core/lark";
import { slackInstallationsOptions } from "@patchbay/core/slack";
import { dingtalkInstallationsOptions } from "@patchbay/core/dingtalk";
import { wecomInstallationsOptions } from "@patchbay/core/wecom";
import { telegramInstallationsOptions } from "@patchbay/core/telegram";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@patchbay/ui/components/ui/alert-dialog";
import { cn } from "@patchbay/ui/lib/utils";
import { EnvTab } from "./tabs/env-tab";
import { CustomArgsTab } from "./tabs/custom-args-tab";
import { IntegrationsTab } from "./tabs/integrations-tab";
import { RuntimeConfigTab } from "./tabs/runtime-config-tab";
import { AgentDetailInspector } from "./agent-detail-inspector";
import { AgentAccessSettings } from "./agent-access-settings";
import { useT } from "../../i18n";
import { useNavigation } from "../../navigation";

export type DetailTab =
  | "composio_mcp"
  | "integrations"
  | "general"
  | "access"
  | "env"
  | "custom_args"
  | "runtime_config";

type SecondaryTab = {
  id: DetailTab;
  labelKey:
    | "composio_mcp"
    | "integrations"
    | "general"
    | "access"
    | "environment"
    | "custom_args"
    | "runtime_config";
};

const SETTINGS_TABS: SecondaryTab[] = [
  { id: "general", labelKey: "general" },
  { id: "access", labelKey: "access" },
  { id: "env", labelKey: "environment" },
  { id: "custom_args", labelKey: "custom_args" },
  { id: "runtime_config", labelKey: "runtime_config" },
  { id: "integrations", labelKey: "integrations" },
];

const DETAIL_VIEWS = new Set<DetailTab>(SETTINGS_TABS.map((tab) => tab.id));
const LEGACY_VIEWS = new Set([
  "overview",
  "work",
  "instructions",
  "skills",
  "mcp_config",
  "composio_mcp",
  "capabilities",
  "settings",
]);

function isDetailTab(value: string | null): value is DetailTab {
  return value !== null && DETAIL_VIEWS.has(value as DetailTab);
}

function viewFromUrl(value: string | null): DetailTab {
  if (isDetailTab(value)) return value;
  if (value !== null && LEGACY_VIEWS.has(value)) return "general";
  return "general";
}

interface AgentOverviewPaneProps {
  agent: Agent;
  runtime: AgentRuntime | null;
  owner: MemberWithUser | null;
  runtimes: AgentRuntime[];
  members: MemberWithUser[];
  onUpdate: (id: string, data: Record<string, unknown>) => Promise<void>;
  currentUserId?: string | null;
  canEdit: boolean;
  navIntent?: DetailTab | null;
  onNavIntentHandled?: () => void;
}

/**
 * Agent settings workbench. Identity lives on the page card; Skills and MCP
 * are workspace-shared in Settings. This pane keeps how the agent runs.
 */
export function AgentOverviewPane({
  agent,
  runtime,
  owner: _owner,
  runtimes,
  members,
  onUpdate,
  currentUserId,
  canEdit,
  navIntent,
  onNavIntentHandled,
}: AgentOverviewPaneProps) {
  const { t } = useT("agents");
  const wsId = useWorkspaceId();
  const navigation = useNavigation();
  const urlView = navigation.searchParams.get("view");
  const [activeView, setActiveView] = useState<DetailTab>(() =>
    viewFromUrl(urlView),
  );
  const [activeDirty, setActiveDirty] = useState(false);
  const [pendingView, setPendingView] = useState<DetailTab | null>(null);
  const lastUrlViewRef = useRef(urlView);

  const { data: larkListing } = useQuery({
    ...larkInstallationsOptions(wsId),
    enabled: !!wsId,
  });
  const { data: slackListing } = useQuery({
    ...slackInstallationsOptions(wsId),
    enabled: !!wsId,
  });
  const { data: dingtalkListing } = useQuery({
    ...dingtalkInstallationsOptions(wsId),
  });
  const { data: wecomListing } = useQuery({
    ...wecomInstallationsOptions(wsId),
    enabled: !!wsId,
  });
  const { data: telegramListing } = useQuery({
    ...telegramInstallationsOptions(wsId),
    enabled: !!wsId,
  });

  const integrationsConfigured =
    larkListing?.configured === true ||
    slackListing?.configured === true ||
    dingtalkListing?.configured === true ||
    wecomListing?.configured === true ||
    telegramListing?.configured === true;

  const visibleSettingsTabs = useMemo(() => {
    return SETTINGS_TABS.filter((tab) => {
      // Env is the only settings tab backed by a secret-bearing endpoint.
      // GET/PUT /api/agents/{id}/env admits the agent owner or a workspace
      // owner/admin (MUL-5438) — the same rule `canEdit` encodes — so
      // showing the tab to anyone else guarantees a 403 on "Reveal & edit".
      if (tab.id === "env") return canEdit;
      if (tab.id === "runtime_config") return runtime?.provider === "openclaw";
      if (tab.id === "integrations") return integrationsConfigured;
      return true;
    });
  }, [canEdit, integrationsConfigured, runtime?.provider]);

  const visibleViews = useMemo(
    () => new Set<DetailTab>(visibleSettingsTabs.map((tab) => tab.id)),
    [visibleSettingsTabs],
  );

  const effectiveView = visibleViews.has(activeView) ? activeView : "general";

  const commitView = useCallback(
    (next: DetailTab) => {
      setActiveView(next);
      const params = new URLSearchParams(navigation.searchParams);
      if (next === "general") params.delete("view");
      else params.set("view", next);
      const query = params.toString();
      navigation.replace(`${navigation.pathname}${query ? `?${query}` : ""}`);
    },
    [navigation],
  );

  const requestView = useCallback(
    (next: DetailTab) => {
      if (next === effectiveView) return;
      if (activeDirty) {
        setPendingView(next);
        return;
      }
      commitView(next);
    },
    [activeDirty, commitView, effectiveView],
  );

  const commitViewChange = () => {
    if (!pendingView) return;
    commitView(pendingView);
    setActiveDirty(false);
    setPendingView(null);
  };

  useEffect(() => {
    if (urlView === lastUrlViewRef.current) return;
    lastUrlViewRef.current = urlView;
    const next = viewFromUrl(urlView);
    if (visibleViews.has(next)) setActiveView(next);
  }, [urlView, visibleViews]);

  useEffect(() => {
    if (navIntent == null) return;
    if (visibleViews.has(navIntent)) requestView(navIntent);
    onNavIntentHandled?.();
  }, [navIntent, onNavIntentHandled, requestView, visibleViews]);

  const activeSecondaryTab = visibleSettingsTabs.find(
    (tab) => tab.id === effectiveView,
  );

  return (
    <div className="flex min-h-0 flex-1 flex-col bg-background">
      <div className="min-h-0 flex-1 overflow-y-auto md:overflow-hidden">
        {visibleSettingsTabs.length > 0 && activeSecondaryTab && (
          <div className="flex min-h-full flex-col md:h-full md:flex-row">
            <aside className="shrink-0 overflow-x-auto border-b border-surface-border p-2 md:w-52 md:overflow-y-auto md:border-b-0 md:border-r md:p-4">
              <div
                className="flex w-max min-w-full items-center gap-1 md:w-full md:flex-col md:items-stretch"
                role="tablist"
                aria-orientation="vertical"
                aria-label={t(($) => $.tabs.section_navigation_aria)}
              >
                {visibleSettingsTabs.map((tab) => {
                  const active = effectiveView === tab.id;
                  return (
                    <button
                      key={tab.id}
                      type="button"
                      role="tab"
                      aria-selected={active}
                      onClick={() => requestView(tab.id)}
                      className={cn(
                        "flex h-8 shrink-0 items-center rounded-md px-2.5 text-left text-caption transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring md:w-full",
                        active
                          ? "bg-surface-selected font-medium text-surface-selected-foreground hover:bg-surface-selected"
                          : "text-muted-foreground hover:bg-surface-hover hover:text-foreground",
                      )}
                    >
                      {t(($) => $.tabs[tab.labelKey])}
                    </button>
                  );
                })}
              </div>
            </aside>

            <section className="min-w-0 flex-1 md:overflow-y-auto">
              <div className="mx-auto w-full max-w-3xl p-4 sm:p-6 md:p-8">
                <header>
                  <h2 className="text-title-sm font-medium text-balance">
                    {t(($) => $.tabs[activeSecondaryTab.labelKey])}
                  </h2>
                </header>

                <div className="mt-6">
                  {effectiveView === "integrations" && (
                    <IntegrationsTab agent={agent} />
                  )}
                  {effectiveView === "general" && (
                    <AgentDetailInspector
                      agent={agent}
                      runtime={runtime}
                      runtimes={runtimes}
                      members={members}
                      currentUserId={currentUserId ?? null}
                      canEdit={canEdit}
                      onUpdate={onUpdate}
                    />
                  )}
                  {effectiveView === "access" && (
                    <AgentAccessSettings
                      agent={agent}
                      members={members}
                      currentUserId={currentUserId ?? null}
                      onDirtyChange={setActiveDirty}
                      onUpdate={onUpdate}
                    />
                  )}
                  {effectiveView === "env" && (
                    <EnvTab agent={agent} onDirtyChange={setActiveDirty} />
                  )}
                  {effectiveView === "custom_args" && (
                    <CustomArgsTab
                      agent={agent}
                      runtimeDevice={runtime ?? undefined}
                      onSave={(updates) => onUpdate(agent.id, updates)}
                      onDirtyChange={setActiveDirty}
                    />
                  )}
                  {effectiveView === "runtime_config" && (
                    <RuntimeConfigTab
                      agent={agent}
                      onSave={(updates) => onUpdate(agent.id, updates)}
                      onDirtyChange={setActiveDirty}
                    />
                  )}
                </div>
              </div>
            </section>
          </div>
        )}
      </div>

      {pendingView !== null && (
        <AlertDialog
          open
          onOpenChange={(open) => {
            if (!open) setPendingView(null);
          }}
        >
          <AlertDialogContent>
            <AlertDialogHeader>
              <AlertDialogTitle>
                {t(($) => $.tabs.discard_dialog_title)}
              </AlertDialogTitle>
              <AlertDialogDescription>
                {t(($) => $.tabs.discard_dialog_description)}
              </AlertDialogDescription>
            </AlertDialogHeader>
            <AlertDialogFooter>
              <AlertDialogCancel>
                {t(($) => $.tabs.discard_keep)}
              </AlertDialogCancel>
              <AlertDialogAction
                variant="destructive"
                onClick={commitViewChange}
              >
                {t(($) => $.tabs.discard_confirm)}
              </AlertDialogAction>
            </AlertDialogFooter>
          </AlertDialogContent>
        </AlertDialog>
      )}
    </div>
  );
}
