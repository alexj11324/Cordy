"use client";

import type { ReactNode } from "react";
import { ChevronDown, FolderKanban } from "lucide-react";
import { useQuery } from "@tanstack/react-query";
import { useWorkspaceId } from "@orvilo/core/hooks";
import { useWorkspacePaths } from "@orvilo/core/paths";
import { projectDetailOptions } from "@orvilo/core/projects/queries";
import type { AutomationStatus } from "@orvilo/core/types";
import { Input } from "@orvilo/ui/components/ui/input";
import { Skeleton } from "@orvilo/ui/components/ui/skeleton";
import { Switch } from "@orvilo/ui/components/ui/switch";
import {
  Tabs,
  TabsContent,
  TabsList,
  TabsTrigger,
} from "@orvilo/ui/components/ui/tabs";
import { cn } from "@orvilo/ui/lib/utils";
import { BreadcrumbHeader } from "../../layout/breadcrumb-header";
import { ProjectPicker } from "../../projects/components/project-picker";
import { ProjectIcon } from "../../projects/components/project-icon";
import { useT } from "../../i18n";

// The one Settings page renderer. Creation supplies local draft values;
// existing automations supply server values and persistence callbacks.
export function AutomationSettingsPage({
  title,
  onTitleChange,
  titleError,
  status,
  creatorName,
  projectId,
  canWrite,
  busy = false,
  onProjectChange,
  onToggleStatus,
  tab,
  onTabChange,
  actions,
  notice,
  children,
  renderRuns,
  inlineRunTabs = false,
}: {
  title: string;
  onTitleChange?: (value: string) => void;
  titleError?: string;
  status: AutomationStatus | "draft";
  creatorName: string;
  projectId: string | null;
  canWrite: boolean;
  busy?: boolean;
  onProjectChange: (projectId: string | null) => void;
  onToggleStatus?: (checked: boolean) => void;
  tab: "settings" | "runs";
  onTabChange?: (value: "settings" | "runs") => void;
  actions?: ReactNode;
  notice?: ReactNode;
  children: ReactNode;
  renderRuns?: (tabs: ReactNode) => ReactNode;
  inlineRunTabs?: boolean;
}) {
  const { t } = useT("automations");
  const wsId = useWorkspaceId();
  const paths = useWorkspacePaths();
  const { data: project, isLoading: projectLoading } = useQuery({
    ...projectDetailOptions(wsId, projectId ?? ""),
    enabled: Boolean(projectId),
  });
  const draft = status === "draft";
  const tabs = (
    <TabsList className="h-7 bg-transparent p-0">
      <TabsTrigger
        value="settings"
        className="h-7 flex-none rounded-md px-2.5 text-label after:hidden data-active:bg-muted data-active:shadow-none"
      >
        {t(($) => $.settings.tab_settings)}
      </TabsTrigger>
      <TabsTrigger
        value="runs"
        disabled={draft}
        className="h-7 flex-none rounded-md px-2.5 text-label after:hidden data-active:bg-muted data-active:shadow-none"
      >
        {t(($) => $.settings.tab_runs)}
      </TabsTrigger>
    </TabsList>
  );
  return (
    <div
      className="flex min-h-0 flex-1 flex-col"
      data-testid="automation-settings-page"
    >
      <BreadcrumbHeader
        segments={[
          { href: paths.automations(), label: t(($) => $.page.title) },
        ]}
        leaf={
          <span className="min-w-0 truncate text-body text-muted-foreground">
            {draft ? t(($) => $.page.new_automation) : title}
          </span>
        }
        actions={actions}
      />
      {notice}
      <Tabs
        value={tab}
        onValueChange={(value) => onTabChange?.(value as "settings" | "runs")}
        className="flex min-h-0 flex-1 flex-col gap-0"
      >
        <div className="flex-1 overflow-y-auto">
          <div className="mx-auto max-w-5xl space-y-8 px-4 py-6 sm:px-8 sm:py-8">
            <header className="space-y-4">
              {onTitleChange ? (
                <Input
                  id="automation-settings-name"
                  data-testid="automation-settings-title"
                  autoFocus
                  disabled={busy}
                  value={title}
                  onChange={(event) => onTitleChange(event.target.value)}
                  aria-label={t(($) => $.page.table.name)}
                  aria-invalid={Boolean(titleError)}
                  aria-describedby={
                    titleError ? "automation-settings-name-error" : undefined
                  }
                  placeholder={t(($) => $.page.new_automation)}
                  className="h-auto rounded-none border-0 bg-transparent dark:bg-transparent px-0 py-0 text-display-sm font-bold leading-snug tracking-tight text-foreground shadow-none placeholder:text-foreground focus-visible:outline-2 focus-visible:outline-offset-4 focus-visible:outline-ring focus-visible:ring-0 md:text-display-sm"
                />
              ) : (
                <h1
                  data-testid="automation-settings-title"
                  className="text-display-sm font-bold leading-snug tracking-tight text-foreground"
                >
                  {title}
                </h1>
              )}
              {titleError ? (
                <p
                  id="automation-settings-name-error"
                  role="alert"
                  className="text-caption text-destructive"
                >
                  {titleError}
                </p>
              ) : null}
              <div
                className="flex flex-wrap items-center gap-x-3 gap-y-2"
                data-testid="automation-settings-metadata"
              >
                <div className="flex items-center gap-1.5">
                  <Switch
                    size="sm"
                    checked={status === "active"}
                    onCheckedChange={onToggleStatus}
                    disabled={
                      draft || status === "archived" || !canWrite || busy
                    }
                    aria-label={
                      status === "active"
                        ? t(($) => $.detail.pause_aria)
                        : t(($) => $.detail.activate_aria)
                    }
                  />
                  <span
                    className={cn(
                      "text-caption font-medium",
                      status === "active"
                        ? "text-emerald-500"
                        : "text-muted-foreground",
                    )}
                  >
                    {draft
                      ? t(($) => $.create_settings.draft)
                      : status === "active"
                        ? t(($) => $.status.active)
                        : t(($) => $.detail.status_inactive)}
                  </span>
                </div>
                <span aria-hidden className="h-3.5 w-px shrink-0 bg-border" />
                <ProjectPicker
                  projectId={projectId}
                  disabled={!canWrite || busy}
                  onUpdate={(updates) => {
                    if (canWrite && updates.project_id !== undefined)
                      onProjectChange(updates.project_id);
                  }}
                  triggerRender={
                    <button
                      type="button"
                      disabled={!canWrite || busy}
                      className="inline-flex h-7 max-w-[14rem] items-center gap-1 rounded-md px-1 text-caption text-muted-foreground hover:bg-accent/30 disabled:pointer-events-none disabled:opacity-50"
                    >
                      {projectLoading ? (
                        <Skeleton className="h-3.5 w-20" />
                      ) : project ? (
                        <>
                          <ProjectIcon project={project} size="sm" />
                          <span className="truncate text-foreground">
                            {project.title}
                          </span>
                        </>
                      ) : (
                        <>
                          <FolderKanban className="size-3.5 shrink-0" />
                          <span className="truncate">
                            {t(($) => $.detail.no_project)}
                          </span>
                        </>
                      )}
                      <ChevronDown className="size-3 shrink-0" />
                    </button>
                  }
                />
                <span aria-hidden className="h-3.5 w-px shrink-0 bg-border" />
                <span className="text-caption text-muted-foreground">
                  {t(($) => $.detail.created_by_line, { name: creatorName })}
                </span>
              </div>
              {tab !== "runs" || !inlineRunTabs ? tabs : null}
            </header>
            <TabsContent value="settings" className="mt-0 max-w-3xl space-y-8">
              {children}
            </TabsContent>
            <TabsContent value="runs" className="mt-0 space-y-3">
              {renderRuns?.(tabs)}
            </TabsContent>
          </div>
        </div>
      </Tabs>
    </div>
  );
}
