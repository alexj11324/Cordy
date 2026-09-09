"use client";

import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { useQuery } from "@tanstack/react-query";
import type { Agent, AgentRuntime, MemberWithUser } from "@orvilo/core/types";
import { useWorkspaceId } from "@orvilo/core/hooks";
import { larkInstallationsOptions } from "@orvilo/core/lark";
import { slackInstallationsOptions } from "@orvilo/core/slack";
import { dingtalkInstallationsOptions } from "@orvilo/core/dingtalk";
import { wecomInstallationsOptions } from "@orvilo/core/wecom";
import { telegramInstallationsOptions } from "@orvilo/core/telegram";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@orvilo/ui/components/ui/alert-dialog";
import {
  Tabs,
  TabsContent,
  TabsList,
  TabsTrigger,
} from "@orvilo/ui/components/ui/tabs";
import { CustomArgsTab } from "./tabs/custom-args-tab";
import { IntegrationsTab } from "./tabs/integrations-tab";
import { RuntimeConfigTab } from "./tabs/runtime-config-tab";
import { AgentDetailInspector } from "./agent-detail-inspector";
import { AgentAccessSettings } from "./agent-access-settings";
import { Badge } from "@orvilo/ui/components/reui/badge";
import { useT } from "../../i18n";
import { useNavigation } from "../../navigation";

export type DetailTab = "general" | "access";

type SecondaryTab = {
  id: DetailTab;
  labelKey: "general" | "access";
};

const SETTINGS_TABS: SecondaryTab[] = [
  { id: "general", labelKey: "general" },
  { id: "access", labelKey: "access" },
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
  "integrations",
  "env",
  "custom_args",
  "runtime_config",
]);

type DirtySection = "access" | "custom_args" | "runtime_config";

const CLEAN_SECTIONS: Record<DirtySection, boolean> = {
  access: false,
  custom_args: false,
  runtime_config: false,
};

function isDetailTab(value: string | null): value is DetailTab {
  return value !== null && DETAIL_VIEWS.has(value as DetailTab);
}

function viewFromUrl(value: string | null): DetailTab {
  if (isDetailTab(value)) return value;
  if (value !== null && LEGACY_VIEWS.has(value)) return "general";
  return "general";
}

/**
 * Page settings use plain sections so their editors own any needed borders.
 */
function SettingsSection({
  title,
  description,
  children,
}: {
  title: string;
  description: string;
  children: ReactNode;
}) {
  return (
    <section className="space-y-3 border-b pb-6 last:border-b-0 last:pb-0">
      <header className="space-y-1">
        <h3 className="text-body font-medium">{title}</h3>
        <p className="text-caption text-muted-foreground">{description}</p>
      </header>
      {children}
    </section>
  );
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
 * Agent settings workbench. Identity lives in the page header; Skills and MCP
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
  const [dirtySections, setDirtySections] = useState(CLEAN_SECTIONS);
  const [pendingView, setPendingView] = useState<DetailTab | null>(null);
  const [pendingUpdate, setPendingUpdate] = useState<{
    id: string;
    data: Record<string, unknown>;
  } | null>(null);
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

  const activeDirty = Object.values(dirtySections).some(Boolean);
  const visibleSettingsTabs = SETTINGS_TABS;
  const visibleViews = DETAIL_VIEWS;

  const handleDirtyChange = useCallback(
    (section: DirtySection, dirty: boolean) => {
      setDirtySections((current) =>
        current[section] === dirty ? current : { ...current, [section]: dirty },
      );
    },
    [],
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

  const requestUpdate = useCallback(
    async (id: string, data: Record<string, unknown>) => {
      const runtimeId =
        typeof data.runtime_id === "string" ? data.runtime_id : null;
      const nextRuntime = runtimeId
        ? runtimes.find((candidate) => candidate.id === runtimeId)
        : null;
      if (
        dirtySections.runtime_config &&
        nextRuntime &&
        nextRuntime.provider !== "openclaw"
      ) {
        setPendingUpdate({ id, data });
        return;
      }
      await onUpdate(id, data);
    },
    [dirtySections.runtime_config, onUpdate, runtimes],
  );

  const commitViewChange = () => {
    if (!pendingView) return;
    commitView(pendingView);
    setDirtySections(CLEAN_SECTIONS);
    setPendingView(null);
  };

  const commitPendingUpdate = async () => {
    if (!pendingUpdate) return;
    const update = pendingUpdate;
    setPendingUpdate(null);
    await onUpdate(update.id, update.data);
    setDirtySections(CLEAN_SECTIONS);
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
  const hasAnyDirty = Object.values(dirtySections).some(Boolean);

  return (
    <div className="flex min-h-0 flex-1 flex-col bg-background">
      <div className="min-h-0 flex-1 overflow-y-auto md:overflow-hidden">
        {visibleSettingsTabs.length > 0 && activeSecondaryTab && (
          <Tabs
            value={effectiveView}
            onValueChange={(value) => requestView(value as DetailTab)}
            className="flex min-h-full flex-col gap-0 md:h-full"
          >
            <div className="flex shrink-0 items-center gap-3 overflow-x-auto border-b px-4 py-2 sm:px-6">
              <TabsList
                className="w-max"
                aria-label={t(($) => $.tabs.section_navigation_aria)}
              >
                {visibleSettingsTabs.map((tab) => (
                  <TabsTrigger
                    key={tab.id}
                    value={tab.id}
                    className="flex-none px-3"
                  >
                    {t(($) => $.tabs[tab.labelKey])}
                  </TabsTrigger>
                ))}
              </TabsList>
              {hasAnyDirty ? (
                <Badge variant="warning-light" size="sm">
                  {t(($) => $.tabs.unsaved)}
                </Badge>
              ) : null}
            </div>

            <section className="min-w-0 flex-1 md:overflow-y-auto">
              <div className="w-full max-w-5xl p-4 sm:p-6">
                <TabsContent value={effectiveView} className="mt-0">
                  {effectiveView === "general" && (
                    <div className="space-y-6">
                      <AgentDetailInspector
                        agent={agent}
                        runtime={runtime}
                        runtimes={runtimes}
                        members={members}
                        currentUserId={currentUserId ?? null}
                        canEdit={canEdit}
                        onUpdate={requestUpdate}
                      />

                      <SettingsSection
                        title={t(($) => $.tabs.custom_args)}
                        description={t(($) => $.tabs.custom_args_hint)}
                      >
                        <CustomArgsTab
                          compact
                          agent={agent}
                          runtimeDevice={runtime ?? undefined}
                          onSave={(updates) => onUpdate(agent.id, updates)}
                          onDirtyChange={(dirty) =>
                            handleDirtyChange("custom_args", dirty)
                          }
                        />
                      </SettingsSection>

                      {runtime?.provider === "openclaw" && (
                        <SettingsSection
                          title={t(($) => $.tabs.runtime_config)}
                          description={t(($) => $.tabs.runtime_config_hint)}
                        >
                          <RuntimeConfigTab
                            agent={agent}
                            onSave={(updates) => onUpdate(agent.id, updates)}
                            onDirtyChange={(dirty) =>
                              handleDirtyChange("runtime_config", dirty)
                            }
                          />
                        </SettingsSection>
                      )}

                      {integrationsConfigured && (
                        <SettingsSection
                          title={t(($) => $.tabs.integrations)}
                          description={t(($) => $.tabs.integrations_hint)}
                        >
                          <IntegrationsTab agent={agent} />
                        </SettingsSection>
                      )}
                    </div>
                  )}
                  {effectiveView === "access" && (
                    <div className="py-2">
                      <AgentAccessSettings
                        agent={agent}
                        members={members}
                        currentUserId={currentUserId ?? null}
                        onDirtyChange={(dirty) =>
                          handleDirtyChange("access", dirty)
                        }
                        onUpdate={onUpdate}
                      />
                    </div>
                  )}
                </TabsContent>
              </div>
            </section>
          </Tabs>
        )}
      </div>

      {(pendingView !== null || pendingUpdate !== null) && (
        <AlertDialog
          open
          onOpenChange={(open) => {
            if (!open) {
              setPendingView(null);
              setPendingUpdate(null);
            }
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
                onClick={pendingUpdate ? commitPendingUpdate : commitViewChange}
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
