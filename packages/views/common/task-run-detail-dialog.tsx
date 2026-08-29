"use client";

import { useMemo, useState, type ReactNode } from "react";
import { AlertCircle, Loader2, ScrollText, Wrench } from "lucide-react";
import { useQuery } from "@tanstack/react-query";
import { api, clientErrorMessage } from "@patchbay/core/api";
import {
  chatKeys,
  mergeTaskMessagesBySeq,
} from "@patchbay/core/chat/queries";
import type { AgentTask, TaskMessagePayload } from "@patchbay/core/types";
import { cn } from "@patchbay/ui/lib/utils";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@patchbay/ui/components/ui/dialog";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@patchbay/ui/components/ui/tooltip";
import { useT } from "../i18n";
import {
  buildTimeline,
  type TimelineItem,
} from "./task-transcript/build-timeline";
import { redactSecrets } from "./task-transcript/redact";

interface TaskRunDetailDialogProps {
  task: AgentTask;
  agentName: string;
  statusLabel?: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  headerSlot?: ReactNode;
}

export interface TaskRunDetailButtonProps {
  task: AgentTask;
  agentName: string;
  statusLabel?: string;
  title?: string;
  className?: string;
  headerSlot?: ReactNode;
}

/**
 * Issue-linked work has its inspection/conversation surface in the issue
 * thread. Only unlinked, non-chat tasks need this fallback action.
 */
export function shouldShowTaskRunDetails(
  task: Pick<AgentTask, "issue_id" | "chat_session_id">,
): boolean {
  return task.issue_id === "" && !task.chat_session_id;
}

function jsonText(value: unknown): string | null {
  if (value === undefined || value === null) return null;
  try {
    return redactSecrets(JSON.stringify(value, null, 2));
  } catch {
    return null;
  }
}

function TimelineEvent({
  item,
}: {
  item: TimelineItem;
}) {
  const { t } = useT("agents");
  const input = item.type === "tool_use" ? jsonText(item.input) : null;
  const output = item.type === "tool_result" ? item.output : null;
  const body = item.content ?? output;
  const label =
    item.type === "text"
      ? t(($) => $.task_detail.text)
      : item.type === "thinking"
        ? t(($) => $.task_detail.thinking)
        : item.type === "tool_use"
          ? t(($) => $.task_detail.tool_use)
          : item.type === "tool_result"
            ? t(($) => $.task_detail.tool_result)
            : t(($) => $.task_detail.error);

  return (
    <li className="flex gap-2.5 border-b border-border/50 px-3 py-2.5 last:border-b-0">
      {item.type === "tool_use" || item.type === "tool_result" ? (
        <Wrench
          className="mt-0.5 h-3.5 w-3.5 shrink-0 text-muted-foreground"
          aria-hidden="true"
        />
      ) : (
        <AlertCircle
          className={cn(
            "mt-0.5 h-3.5 w-3.5 shrink-0",
            item.type === "error" ? "text-destructive" : "text-muted-foreground",
          )}
          aria-hidden="true"
        />
      )}
      <div className="min-w-0 flex-1">
        <div className="flex items-baseline gap-2 text-caption">
          <span className="font-medium">{label}</span>
          {item.tool && (
            <code className="truncate text-muted-foreground">{item.tool}</code>
          )}
          {item.created_at && (
            <time className="ml-auto shrink-0 text-micro text-muted-foreground">
              {new Date(item.created_at).toLocaleTimeString()}
            </time>
          )}
        </div>
        {body && (
          <pre className="mt-1 max-h-48 overflow-auto whitespace-pre-wrap break-words font-sans text-caption text-muted-foreground">
            {redactSecrets(body)}
          </pre>
        )}
        {input && (
          <details className="mt-1 text-caption">
            <summary className="cursor-pointer text-muted-foreground hover:text-foreground">
              {t(($) => $.task_detail.input)}
            </summary>
            <pre className="mt-1 max-h-40 overflow-auto whitespace-pre-wrap break-words rounded bg-muted/40 p-2 font-mono text-micro text-muted-foreground">
              {input}
            </pre>
          </details>
        )}
      </div>
    </li>
  );
}

/**
 * Read-only fallback inspection surface for runs that do not have an issue
 * thread. Issue-linked runs deliberately do not use this component: their
 * live conversation is rendered in the issue thread instead.
 */
export function TaskRunDetailDialog({
  task,
  agentName,
  statusLabel,
  open,
  onOpenChange,
  headerSlot,
}: TaskRunDetailDialogProps) {
  const { t } = useT("agents");
  const query = useQuery({
    queryKey: chatKeys.taskMessages(task.id),
    queryFn: () => api.listTaskMessages(task.id),
    enabled: open && task.id.length > 0,
    staleTime: Infinity,
    structuralSharing: (previous, next) =>
      mergeTaskMessagesBySeq(
        Array.isArray(previous) ? (previous as TaskMessagePayload[]) : [],
        next as TaskMessagePayload[],
      ),
  });
  const items = useMemo(() => buildTimeline(query.data ?? []), [query.data]);
  const displayStatus = statusLabel ?? task.status;
  const loadError = query.error
    ? clientErrorMessage(query.error) || t(($) => $.task_detail.load_failed)
    : null;

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        data-testid="task-run-detail-dialog"
        className="grid max-h-[calc(100vh-2rem)] grid-rows-[auto_minmax(0,1fr)] max-w-2xl overflow-hidden"
      >
        <DialogHeader>
          <DialogTitle>{t(($) => $.task_detail.title)}</DialogTitle>
          <DialogDescription>
            {t(($) => $.task_detail.description, {
              agent: agentName,
              status: displayStatus,
            })}
          </DialogDescription>
        </DialogHeader>
        <div className="min-h-0 min-w-0 space-y-3 overflow-y-auto">
          {headerSlot}
          {query.isLoading ? (
            <div className="flex items-center gap-2 rounded-md border px-3 py-4 text-caption text-muted-foreground">
              <Loader2 className="h-3.5 w-3.5 animate-spin" aria-hidden="true" />
              {t(($) => $.task_detail.loading)}
            </div>
          ) : loadError ? (
            <p className="rounded-md border border-destructive/30 bg-destructive/5 px-3 py-3 text-caption text-destructive">
              {loadError}
            </p>
          ) : items.length === 0 ? (
            <p className="rounded-md border border-dashed px-3 py-4 text-center text-caption text-muted-foreground">
              {t(($) => $.task_detail.empty)}
            </p>
          ) : (
            <>
              <div className="flex items-center justify-between text-micro text-muted-foreground">
                <span>{t(($) => $.task_detail.events, { count: items.length })}</span>
                <span className="font-mono">{task.id}</span>
              </div>
              <ol
                data-testid="task-run-detail-events"
                className="overflow-hidden rounded-md border bg-background"
              >
                {items.map((item) => (
                  <TimelineEvent key={`${item.seq}:${item.type}`} item={item} />
                ))}
              </ol>
            </>
          )}
        </div>
      </DialogContent>
    </Dialog>
  );
}

/** Compact entry action used only by non-issue task/run rows. */
export function TaskRunDetailButton({
  task,
  agentName,
  statusLabel,
  title,
  className,
  headerSlot,
}: TaskRunDetailButtonProps) {
  const { t } = useT("agents");
  const [open, setOpen] = useState(false);
  const label = title ?? t(($) => $.task_detail.open_tooltip);

  return (
    <>
      <Tooltip>
        <TooltipTrigger
          render={<button type="button" />}
          onClick={(event) => {
            event.preventDefault();
            event.stopPropagation();
            setOpen(true);
          }}
          aria-label={label}
          data-testid="task-run-detail-trigger"
          className={cn(
            "flex items-center justify-center rounded p-1 text-muted-foreground hover:bg-accent/50 hover:text-foreground transition-colors",
            className,
          )}
        >
          <ScrollText className="h-3.5 w-3.5" aria-hidden="true" />
        </TooltipTrigger>
        <TooltipContent>{label}</TooltipContent>
      </Tooltip>
      {open && (
        <TaskRunDetailDialog
          task={task}
          agentName={agentName}
          statusLabel={statusLabel}
          open={open}
          onOpenChange={setOpen}
          headerSlot={headerSlot}
        />
      )}
    </>
  );
}
