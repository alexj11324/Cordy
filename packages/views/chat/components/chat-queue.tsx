"use client";

import { useState } from "react";
import { ArrowUp, ListEnd, Loader2, Pencil, Trash2 } from "lucide-react";
import type { ChatQueuedTask } from "@patchbay/core/types";
import { Button } from "@patchbay/ui/components/ui/button";
import { cn } from "@patchbay/ui/lib/utils";
import { useT } from "../../i18n";

interface ChatQueueProps {
  tasks: ChatQueuedTask[];
  headStatus: string | undefined;
  onSendNow?: (taskId: string) => Promise<void> | void;
  /** Blocks "send now" independently of the head task's status — used when the
   *  caller may no longer invoke the agent, since steering a queued task
   *  dispatches a run the server would refuse (PB-6380). */
  sendNowDisabled?: boolean;
  /** Render the queue without mutation controls when the owner only has read access. */
  readOnly?: boolean;
  onEdit?: (taskId: string) => Promise<void> | void;
  onRemove?: (taskId: string) => Promise<void> | void;
  onClear?: () => Promise<void> | void;
}

export function ChatQueue({
  tasks,
  headStatus,
  onSendNow,
  sendNowDisabled = false,
  readOnly = false,
  onEdit,
  onRemove,
  onClear,
}: ChatQueueProps) {
  const { t } = useT("chat");
  const [busyAction, setBusyAction] = useState<string | null>(null);
  const dispatchableHead =
    headStatus === "dispatched" ||
    headStatus === "running" ||
    headStatus === "waiting_local_directory";
  const canSendNow =
    !readOnly && !!onSendNow && !sendNowDisabled && dispatchableHead;
  const sendNowLabel = t(($) =>
    canSendNow
      ? $.queue.steer
      : sendNowDisabled
        ? $.queue.steer_no_permission
        : $.queue.steer_unavailable,
  );

  if (tasks.length === 0) return null;

  const run = async (key: string, action: () => Promise<void> | void) => {
    setBusyAction(key);
    try {
      await action();
    } finally {
      setBusyAction((current) => (current === key ? null : current));
    }
  };

  return (
    <div
      data-slot="chat-queue-shell"
      className="relative"
      aria-live="polite"
      aria-busy={busyAction !== null}
    >
      <section
        data-slot="chat-queue"
        aria-label={t(($) => $.queue.title, { count: tasks.length })}
        className="border-b border-surface-border bg-surface-raised"
      >
        <div className="flex items-center gap-1.5 px-3 pt-2 text-caption text-muted-foreground">
          <ListEnd
            data-slot="chat-queue-count-icon"
            className="size-3.5 shrink-0"
            aria-hidden="true"
          />
          <span className="min-w-0 flex-1 tabular-nums">{tasks.length}</span>
          {!readOnly && tasks.length > 1 && onClear ? (
            <Button
              type="button"
              variant="ghost"
              size="xs"
              className="h-6 px-1.5 font-normal text-muted-foreground"
              disabled={busyAction !== null}
              aria-label={t(($) => $.queue.clear)}
              onClick={() => void run("clear", onClear)}
            >
              {busyAction === "clear" ? (
                <Loader2 className="animate-spin" aria-hidden="true" />
              ) : (
                t(($) => $.queue.clear)
              )}
            </Button>
          ) : null}
        </div>
        <div
          data-slot="chat-queue-list"
          className="max-h-40 overflow-y-auto px-1 pb-1"
        >
          {tasks.map((task, index) => {
            const sendNowKey = `send-now:${task.task_id}`;
            const editKey = `edit:${task.task_id}`;
            const removeKey = `remove:${task.task_id}`;
            return (
              <div
                key={task.task_id}
                data-slot="chat-queue-row"
                className={cn(
                  "flex min-h-8 min-w-0 items-center gap-1.5 px-2 py-1 text-caption",
                  index > 0 && "border-t border-surface-border/70",
                )}
              >
                <ListEnd
                  data-slot="chat-queue-item-icon"
                  className="size-3.5 shrink-0 text-faint-foreground"
                  aria-hidden="true"
                />
                <span className="min-w-0 flex-1 truncate text-muted-foreground">
                  {task.content?.trim() || t(($) => $.queue.fallback)}
                </span>
                {!readOnly ? (
                  <div className="flex shrink-0 items-center">
                    {onEdit ? (
                      <Button
                        type="button"
                        variant="ghost"
                        size="icon-xs"
                        className="text-muted-foreground"
                        disabled={busyAction !== null}
                        title={t(($) => $.queue.edit)}
                        aria-label={t(($) => $.queue.edit)}
                        onClick={() =>
                          void run(editKey, () => onEdit(task.task_id))
                        }
                      >
                        {busyAction === editKey ? (
                          <Loader2
                            className="animate-spin"
                            aria-hidden="true"
                          />
                        ) : (
                          <Pencil aria-hidden="true" />
                        )}
                      </Button>
                    ) : null}
                    {onSendNow ? (
                      <Button
                        type="button"
                        variant="ghost"
                        size="icon-xs"
                        className="text-muted-foreground"
                        disabled={busyAction !== null || !canSendNow}
                        title={sendNowLabel}
                        aria-label={sendNowLabel}
                        onClick={() =>
                          void run(sendNowKey, () => onSendNow(task.task_id))
                        }
                      >
                        {busyAction === sendNowKey ? (
                          <Loader2
                            className="animate-spin"
                            aria-hidden="true"
                          />
                        ) : (
                          <ArrowUp aria-hidden="true" />
                        )}
                      </Button>
                    ) : null}
                    {onRemove ? (
                      <Button
                        type="button"
                        variant="ghost"
                        size="icon-xs"
                        className="text-muted-foreground"
                        disabled={busyAction !== null}
                        title={t(($) => $.queue.remove)}
                        aria-label={t(($) => $.queue.remove)}
                        onClick={() =>
                          void run(removeKey, () => onRemove(task.task_id))
                        }
                      >
                        {busyAction === removeKey ? (
                          <Loader2
                            className="animate-spin"
                            aria-hidden="true"
                          />
                        ) : (
                          <Trash2 aria-hidden="true" />
                        )}
                      </Button>
                    ) : null}
                  </div>
                ) : null}
              </div>
            );
          })}
        </div>
      </section>
    </div>
  );
}
