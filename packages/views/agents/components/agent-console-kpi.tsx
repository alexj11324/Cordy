"use client";

import { useMemo } from "react";
import {
  Activity,
  Bot,
  CheckCircle2,
  Clock,
  Radio,
} from "lucide-react";
import { Badge } from "@orvilo/ui/components/reui/badge";
import { useLocale, useT } from "../../i18n";
import type { AgentListRow } from "./agents-page";

interface AgentConsoleKpiProps {
  rows: AgentListRow[];
}

export function AgentConsoleKpi({ rows }: AgentConsoleKpiProps) {
  const { t } = useT("agents");
  const locale = useLocale();

  const metrics = useMemo(() => {
    let working = 0;
    let queued = 0;
    let idle = 0;
    let offline = 0;
    let totalRuns = 0;

    for (const r of rows) {
      totalRuns += r.runCount;
      const avail = r.presence?.availability;
      const workload = r.presence?.workload;

      if (workload === "working") {
        working++;
      } else if (workload === "queued") {
        queued++;
      } else if (avail === "online") {
        idle++;
      } else if (avail === "offline" || avail === "unstable") {
        offline++;
      } else {
        // Fallback for agents with no presence yet
        idle++;
      }
    }

    return {
      total: rows.length,
      working,
      queued,
      idle,
      offline,
      totalRuns,
    };
  }, [rows]);

  return (
    <div className="grid grid-cols-2 gap-2.5 sm:grid-cols-3 lg:grid-cols-5">
      {/* 1. Total Agents */}
      <div className="flex items-center gap-3 rounded-xl border border-border/70 bg-card/50 p-3 shadow-2xs backdrop-blur-xs transition-colors hover:border-border hover:bg-card/80">
        <div className="flex size-9 shrink-0 items-center justify-center rounded-lg bg-muted text-muted-foreground">
          <Bot className="size-4.5" />
        </div>
        <div className="min-w-0 flex-1">
          <div className="flex items-center justify-between gap-1">
            <span className="truncate text-caption text-muted-foreground">
              {t(($) => $.kpi.total_agents)}
            </span>
          </div>
          <p className="text-xl font-semibold tracking-tight text-foreground tabular-nums">
            {metrics.total.toLocaleString(locale)}
          </p>
        </div>
      </div>

      {/* 2. Working (Active) */}
      <div className="flex items-center gap-3 rounded-xl border border-emerald-500/20 bg-emerald-500/5 p-3 shadow-2xs transition-colors hover:border-emerald-500/30 hover:bg-emerald-500/8">
        <div className="flex size-9 shrink-0 items-center justify-center rounded-lg bg-emerald-500/15 text-emerald-600 dark:text-emerald-400">
          <Radio className="size-4.5 animate-pulse" />
        </div>
        <div className="min-w-0 flex-1">
          <div className="flex items-center justify-between gap-1">
            <span className="truncate text-caption text-muted-foreground">
              {t(($) => $.kpi.working)}
            </span>
            {metrics.working > 0 && (
              <Badge variant="outline" className="border-emerald-500/40 text-emerald-600 dark:text-emerald-400 px-1.5 py-0 text-[10px]">
                live
              </Badge>
            )}
          </div>
          <p className="text-xl font-semibold tracking-tight text-foreground tabular-nums">
            {metrics.working.toLocaleString(locale)}
          </p>
        </div>
      </div>

      {/* 3. Queued */}
      <div className="flex items-center gap-3 rounded-xl border border-amber-500/20 bg-amber-500/5 p-3 shadow-2xs transition-colors hover:border-amber-500/30 hover:bg-amber-500/8">
        <div className="flex size-9 shrink-0 items-center justify-center rounded-lg bg-amber-500/15 text-amber-600 dark:text-amber-400">
          <Clock className="size-4.5" />
        </div>
        <div className="min-w-0 flex-1">
          <span className="truncate text-caption text-muted-foreground">
            {t(($) => $.kpi.queued)}
          </span>
          <p className="text-xl font-semibold tracking-tight text-foreground tabular-nums">
            {metrics.queued.toLocaleString(locale)}
          </p>
        </div>
      </div>

      {/* 4. Idle & Ready */}
      <div className="flex items-center gap-3 rounded-xl border border-border/70 bg-card/50 p-3 shadow-2xs backdrop-blur-xs transition-colors hover:border-border hover:bg-card/80">
        <div className="flex size-9 shrink-0 items-center justify-center rounded-lg bg-blue-500/10 text-blue-600 dark:text-blue-400">
          <CheckCircle2 className="size-4.5" />
        </div>
        <div className="min-w-0 flex-1">
          <span className="truncate text-caption text-muted-foreground">
            {t(($) => $.kpi.idle)}
          </span>
          <p className="text-xl font-semibold tracking-tight text-foreground tabular-nums">
            {metrics.idle.toLocaleString(locale)}
          </p>
        </div>
      </div>

      {/* 5. 30d Total Runs */}
      <div className="col-span-2 flex items-center gap-3 rounded-xl border border-border/70 bg-card/50 p-3 shadow-2xs backdrop-blur-xs transition-colors hover:border-border hover:bg-card/80 sm:col-span-1">
        <div className="flex size-9 shrink-0 items-center justify-center rounded-lg bg-violet-500/10 text-violet-600 dark:text-violet-400">
          <Activity className="size-4.5" />
        </div>
        <div className="min-w-0 flex-1">
          <span className="truncate text-caption text-muted-foreground">
            {t(($) => $.kpi.runs_30d)}
          </span>
          <p className="text-xl font-semibold tracking-tight text-foreground tabular-nums">
            {metrics.totalRuns.toLocaleString(locale)}
          </p>
        </div>
      </div>
    </div>
  );
}
