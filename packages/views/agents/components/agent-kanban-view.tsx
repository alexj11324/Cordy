"use client";

import { useMemo, useState, useEffect } from "react";
import {
  Bot,
  CheckCircle2,
  Clock,
  ExternalLink,
  Radio,
  WifiOff,
} from "lucide-react";
import {
  Kanban,
  KanbanBoard,
  KanbanColumn,
  KanbanColumnContent,
  KanbanColumnHandle,
  KanbanItem,
  KanbanOverlay,
} from "@orvilo/ui/components/reui/kanban";
import { Badge } from "@orvilo/ui/components/reui/badge";
import { Button } from "@orvilo/ui/components/ui/button";
import { cn } from "@orvilo/ui/lib/utils";
import { useWorkspacePaths } from "@orvilo/core/paths";
import { ActorAvatar } from "../../common/actor-avatar";
import { useNavigation } from "../../navigation";
import { useT } from "../../i18n";
import { availabilityConfig } from "../presence";
import { AgentRowActions } from "./agent-row-actions";
import type { AgentListRow } from "./agents-page";

export type KanbanColumnKey = "working" | "queued" | "idle" | "offline" | "archived";

interface AgentKanbanViewProps {
  rows: AgentListRow[];
  selectedIds: ReadonlySet<string>;
  onToggleSelected: (id: string) => void;
  onOpenSummary: (id: string) => void;
  duplicateHref: (agent: AgentListRow["agent"]) => string;
}

function resolveAgentLane(row: AgentListRow): KanbanColumnKey {
  if (row.agent.archived_at) return "archived";

  const workload = row.presence?.workload;
  const avail = row.presence?.availability;

  if (workload === "working") return "working";
  if (workload === "queued") return "queued";
  if (avail === "offline" || avail === "unstable" || !row.runtime) return "offline";
  return "idle";
}

interface ColumnConfig {
  key: KanbanColumnKey;
  labelKey: "column_working" | "column_queued" | "column_idle" | "column_offline" | "column_archived";
  descKey: "column_working_desc" | "column_queued_desc" | "column_idle_desc" | "column_offline_desc" | "column_archived_desc";
  icon: React.ComponentType<{ className?: string }>;
  accentColor: string;
  badgeClass: string;
}

const COLUMN_CONFIGS: ColumnConfig[] = [
  {
    key: "working",
    labelKey: "column_working",
    descKey: "column_working_desc",
    icon: Radio,
    accentColor: "border-emerald-500/30 bg-emerald-500/5",
    badgeClass: "bg-emerald-500/10 text-emerald-600 dark:text-emerald-400 border-emerald-500/30",
  },
  {
    key: "queued",
    labelKey: "column_queued",
    descKey: "column_queued_desc",
    icon: Clock,
    accentColor: "border-amber-500/30 bg-amber-500/5",
    badgeClass: "bg-amber-500/10 text-amber-600 dark:text-amber-400 border-amber-500/30",
  },
  {
    key: "idle",
    labelKey: "column_idle",
    descKey: "column_idle_desc",
    icon: CheckCircle2,
    accentColor: "border-blue-500/30 bg-blue-500/5",
    badgeClass: "bg-blue-500/10 text-blue-600 dark:text-blue-400 border-blue-500/30",
  },
  {
    key: "offline",
    labelKey: "column_offline",
    descKey: "column_offline_desc",
    icon: WifiOff,
    accentColor: "border-zinc-500/20 bg-zinc-500/5",
    badgeClass: "bg-zinc-500/10 text-muted-foreground border-zinc-500/20",
  },
  {
    key: "archived",
    labelKey: "column_archived",
    descKey: "column_archived_desc",
    icon: Bot,
    accentColor: "border-purple-500/20 bg-purple-500/5",
    badgeClass: "bg-purple-500/10 text-purple-600 dark:text-purple-400 border-purple-500/20",
  },
];

export function AgentKanbanView({
  rows,
  selectedIds,
  onToggleSelected: _onToggleSelected,
  onOpenSummary,
  duplicateHref,
}: AgentKanbanViewProps) {
  const { t } = useT("agents");

  // Group incoming filtered rows into columns
  const initialColumns = useMemo<Record<string, AgentListRow[]>>(() => {
    const cols: Record<KanbanColumnKey, AgentListRow[]> = {
      working: [],
      queued: [],
      idle: [],
      offline: [],
      archived: [],
    };
    for (const r of rows) {
      const lane = resolveAgentLane(r);
      cols[lane].push(r);
    }
    return cols;
  }, [rows]);

  const [columns, setColumns] = useState<Record<string, AgentListRow[]>>(initialColumns);

  useEffect(() => {
    setColumns(initialColumns);
  }, [initialColumns]);

  // If viewing archived scope exclusively, or only standard active
  const activeConfigs = useMemo(() => {
    const hasArchived = (columns.archived?.length ?? 0) > 0;
    if (hasArchived && (columns.working?.length ?? 0) === 0 && (columns.idle?.length ?? 0) === 0) {
      return COLUMN_CONFIGS.filter((c) => c.key === "archived");
    }
    return COLUMN_CONFIGS.filter((c) => c.key !== "archived" || (columns.archived?.length ?? 0) > 0);
  }, [columns]);

  return (
    <div className="flex h-full min-h-0 flex-1 flex-col overflow-hidden">
      <Kanban
        className="flex h-full min-h-0 flex-1 flex-col"
        value={columns}
        onValueChange={setColumns}
        getItemValue={(row) => row.agent.id}
      >
        <KanbanBoard className="flex h-full min-h-0 flex-1 gap-4 overflow-x-auto p-4 sm:p-6">
          {activeConfigs.map((col) => {
            const items = columns[col.key] ?? [];
            const Icon = col.icon;
            const title = t(($) => $[`kanban`][col.labelKey]);
            const desc = t(($) => $[`kanban`][col.descKey]);

            return (
              <KanbanColumn
                key={col.key}
                value={col.key}
                className="flex h-full w-80 shrink-0 flex-col rounded-2xl border border-border/70 bg-card/40 backdrop-blur-xs shadow-2xs"
              >
                {/* Column Header */}
                <KanbanColumnHandle className="flex flex-col gap-1 border-b border-border/60 p-3.5">
                  <div className="flex items-center justify-between gap-2">
                    <div className="flex items-center gap-2 min-w-0">
                      <div className={cn("flex size-6 shrink-0 items-center justify-center rounded-md border", col.badgeClass)}>
                        <Icon className="size-3.5" />
                      </div>
                      <h3 className="truncate text-body font-semibold text-foreground">
                        {title}
                      </h3>
                    </div>
                    <Badge variant="outline" className="border-border/60 text-caption font-semibold tabular-nums">
                      {items.length}
                    </Badge>
                  </div>
                  <p className="truncate text-caption text-muted-foreground">
                    {desc}
                  </p>
                </KanbanColumnHandle>

                {/* Column Card List */}
                <KanbanColumnContent
                  value={col.key}
                  className="flex flex-1 flex-col gap-3 overflow-y-auto p-3"
                >
                  {items.length === 0 ? (
                    <div className="flex flex-1 items-center justify-center rounded-xl border border-dashed border-border/60 p-6 text-center text-caption text-muted-foreground">
                      {t(($) => $.kanban.empty_column)}
                    </div>
                  ) : (
                    items.map((row) => (
                      <KanbanItem key={row.agent.id} value={row.agent.id}>
                        <AgentKanbanCard
                          row={row}
                          selected={selectedIds.has(row.agent.id)}
                          onOpenSummary={() => onOpenSummary(row.agent.id)}
                          duplicateHref={duplicateHref(row.agent)}
                        />
                      </KanbanItem>
                    ))
                  )}
                </KanbanColumnContent>
              </KanbanColumn>
            );
          })}
        </KanbanBoard>

        <KanbanOverlay>
          <div className="h-40 w-72 rounded-xl border border-primary/40 bg-card/90 shadow-lg backdrop-blur-sm" />
        </KanbanOverlay>
      </Kanban>
    </div>
  );
}

interface AgentKanbanCardProps {
  row: AgentListRow;
  selected: boolean;
  onOpenSummary: () => void;
  duplicateHref: string;
}

function AgentKanbanCard({
  row,
  selected,
  onOpenSummary,
  duplicateHref,
}: AgentKanbanCardProps) {
  const { t } = useT("agents");
  const navigation = useNavigation();
  const paths = useWorkspacePaths();

  const avail = row.presence?.availability ?? "offline";
  const visual = availabilityConfig[avail];

  return (
    <div
      onClick={onOpenSummary}
      className={cn(
        "group relative flex cursor-pointer flex-col gap-3 rounded-xl border p-3.5 shadow-2xs transition-all",
        "bg-card/90 hover:border-border hover:shadow-xs",
        selected
          ? "border-primary/60 bg-primary/5 ring-1 ring-primary/20"
          : "border-border/70",
      )}
    >
      {/* Header: Avatar, Name, Owner, Menu */}
      <div className="flex items-start justify-between gap-2.5">
        <div className="flex items-center gap-2.5 min-w-0 flex-1">
          <div className="relative shrink-0">
            <ActorAvatar
              actorId={row.agent.id}
              actorType="agent"
              className="bg-background shadow-xs"
              profileLink={false}
              size="sm"
            />
            {visual ? (
              <span
                className={cn(
                  "absolute -bottom-0.5 -right-0.5 size-2.5 rounded-full ring-2 ring-card",
                  visual.dotClass,
                )}
              />
            ) : null}
          </div>
          <div className="min-w-0 flex-1">
            <p className="truncate text-body font-semibold text-foreground leading-snug">
              {row.agent.name}
            </p>
            {row.owner ? (
              <p className="truncate text-caption text-muted-foreground">
                {row.owner.name || row.owner.email}
              </p>
            ) : null}
          </div>
        </div>

        <div
          className="shrink-0"
          onClick={(e) => e.stopPropagation()}
        >
          <AgentRowActions
            agent={row.agent}
            presence={row.presence}
            canManage={row.canManage}
            duplicateHref={duplicateHref}
          />
        </div>
      </div>

      {/* Badges: Model & Runtime */}
      <div className="flex flex-wrap items-center gap-1.5">
        {row.agent.model ? (
          <Badge
            variant="outline"
            className="border-border/60 bg-muted/40 font-mono text-[11px] text-foreground"
          >
            {row.agent.model}
          </Badge>
        ) : null}

        {row.runtime ? (
          <Badge
            variant="outline"
            className="border-border/50 text-[11px] text-muted-foreground"
          >
            {row.runtime.name}
          </Badge>
        ) : (
          <Badge
            variant="outline"
            className="border-amber-500/30 text-[11px] text-amber-600 dark:text-amber-400"
          >
            {t(($) => $.row.needs_runtime)}
          </Badge>
        )}
      </div>

      {/* Footer: Runs & Quick Action Buttons */}
      <div className="flex items-center justify-between gap-2 pt-1 border-t border-border/50">
        <span className="text-caption text-muted-foreground tabular-nums">
          {row.runCount > 0
            ? t(($) => $.kanban.runs_count, { count: row.runCount })
            : t(($) => $.kanban.no_runs)}
        </span>

        <div
          className="flex items-center gap-1 opacity-80 group-hover:opacity-100 transition-opacity"
          onClick={(e) => e.stopPropagation()}
        >
          <Button
            size="sm"
            variant="ghost"
            className="h-7 px-2 text-caption text-muted-foreground hover:text-foreground"
            onClick={() => navigation.push(paths.agentDetail(row.agent.id))}
          >
            <ExternalLink className="mr-1 size-3" />
            {t(($) => $.kanban.inspect_button)}
          </Button>
        </div>
      </div>
    </div>
  );
}
