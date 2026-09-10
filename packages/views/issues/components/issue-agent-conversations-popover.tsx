"use client";

import { useEffect, useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { MessagesSquare } from "lucide-react";
import { api } from "@orvilo/core/api";
import { issueKeys } from "@orvilo/core/issues/queries";
import { useWorkspaceId } from "@orvilo/core/hooks";
import { agentListOptions } from "@orvilo/core/workspace/queries";
import type { Agent, AgentTask } from "@orvilo/core/types";
import { Button } from "@orvilo/ui/components/ui/button";
import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from "@orvilo/ui/components/ui/command";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@orvilo/ui/components/ui/popover";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@orvilo/ui/components/ui/tooltip";
import { ActorAvatar } from "../../common/actor-avatar";
import { matchesPinyin } from "../../editor/extensions/pinyin-match";
import { useLocale, useT } from "../../i18n";
import { TaskAgentThreadPanel } from "../../agent-thread/components/task-agent-thread-panel";

const ACTIVE_STATUS_RANK: Record<AgentTask["status"], number> = {
  running: 0,
  queued: 1,
  deferred: 1,
  dispatched: 1,
  waiting_local_directory: 1,
  completed: 2,
  failed: 2,
  cancelled: 2,
};

export interface IssueAgentConversation {
  task: AgentTask;
  agent: Agent | undefined;
}

/**
 * One row per Agent. Prefer that Agent's active continuation, otherwise its
 * newest task. Passing a continuation task to the thread endpoint loads its
 * complete chain, so one Agent never appears several times for its own turns.
 */
export function selectIssueAgentConversations(
  tasks: AgentTask[],
  agents: Agent[],
  issueId: string,
): IssueAgentConversation[] {
  const agentById = new Map(agents.map((agent) => [agent.id, agent]));
  const selectedByAgent = new Map<string, AgentTask>();
  for (const task of tasks) {
    if (task.issue_id !== issueId) continue;
    const current = selectedByAgent.get(task.agent_id);
    if (
      !current ||
      ACTIVE_STATUS_RANK[task.status] < ACTIVE_STATUS_RANK[current.status] ||
      (ACTIVE_STATUS_RANK[task.status] === ACTIVE_STATUS_RANK[current.status] &&
        Date.parse(task.created_at) > Date.parse(current.created_at))
    ) {
      selectedByAgent.set(task.agent_id, task);
    }
  }
  return [...selectedByAgent.values()]
    .map((task) => ({ task, agent: agentById.get(task.agent_id) }))
    .sort(
      (left, right) =>
        ACTIVE_STATUS_RANK[left.task.status] - ACTIVE_STATUS_RANK[right.task.status] ||
        Date.parse(right.task.created_at) - Date.parse(left.task.created_at),
    );
}

function statusLabel(status: AgentTask["status"], t: ReturnType<typeof useT<"issues">>["t"]) {
  switch (status) {
    case "running": return t(($) => $.execution_log.status_running);
    case "dispatched": return t(($) => $.execution_log.status_dispatched);
    case "waiting_local_directory": return t(($) => $.execution_log.status_waiting_local_directory);
    case "completed": return t(($) => $.execution_log.status_completed);
    case "failed": return t(($) => $.execution_log.status_failed);
    case "cancelled": return t(($) => $.execution_log.status_cancelled);
    case "queued":
    case "deferred":
      return t(($) => $.execution_log.status_queued);
  }
}

export function IssueAgentConversationsPopover({
  issueId,
  open,
  onOpenChange,
}: {
  issueId: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const { t } = useT("issues");
  const locale = useLocale();
  const workspaceId = useWorkspaceId();
  const [query, setQuery] = useState("");
  const [selectedTaskId, setSelectedTaskId] = useState<string | null>(null);
  useEffect(() => {
    setSelectedTaskId(null);
    setQuery("");
  }, [issueId]);
  const { data: tasks = [], isPending, isError } = useQuery({
    queryKey: issueKeys.tasks(issueId),
    queryFn: () => api.listTasksByIssue(issueId),
    staleTime: 30_000,
    refetchOnWindowFocus: true,
  });
  const { data: agents = [] } = useQuery({
    ...agentListOptions(workspaceId),
    enabled: !!workspaceId,
  });
  const conversations = useMemo(
    () => selectIssueAgentConversations(tasks, agents, issueId),
    [agents, issueId, tasks],
  );
  const filtered = useMemo(() => {
    const needle = query.trim().toLowerCase();
    if (!needle) return conversations;
    return conversations.filter(({ task, agent }) => {
      const name = agent?.name ?? task.agent_id;
      return name.toLowerCase().includes(needle) || matchesPinyin(name, needle);
    });
  }, [conversations, query]);

  const close = () => {
    setSelectedTaskId(null);
    setQuery("");
    onOpenChange(false);
  };
  const label = t(($) => $.detail.agent_conversations.button_label, {
    count: conversations.length,
  });

  return (
    <Popover
      open={open}
      onOpenChange={(next) => {
        if (!next) close();
        else onOpenChange(true);
      }}
    >
      <Tooltip>
        <TooltipTrigger
          render={
            <PopoverTrigger
              render={
                <Button
                  variant={open ? "secondary" : "ghost"}
                  size="sm"
                  aria-label={label}
                  className="h-7 gap-1.5 px-2 text-muted-foreground"
                />
              }
            >
              <MessagesSquare aria-hidden="true" />
              {conversations.length > 0 ? (
                <span className="text-caption font-medium tabular-nums">
                  {conversations.length}
                </span>
              ) : null}
            </PopoverTrigger>
          }
        />
        <TooltipContent side="bottom">{label}</TooltipContent>
      </Tooltip>

      <PopoverContent
        align="end"
        side="bottom"
        initialFocus={false}
        className="h-[min(34rem,calc(100vh-5rem))] w-[360px] overflow-hidden p-0"
      >
        {selectedTaskId ? (
          <TaskAgentThreadPanel
            workspaceId={workspaceId}
            taskId={selectedTaskId}
            onClose={() => setSelectedTaskId(null)}
            closeIcon="back"
            compact
          />
        ) : (
          <Command shouldFilter={false} className="h-full">
            <CommandInput
              value={query}
              onValueChange={setQuery}
              placeholder={t(($) => $.detail.agent_conversations.search_placeholder)}
            />
            <CommandList className="max-h-none flex-1">
              <CommandEmpty>
                {isPending
                  ? t(($) => $.detail.agent_conversations.loading)
                  : isError
                    ? t(($) => $.detail.agent_conversations.load_failed)
                    : t(($) => $.detail.agent_conversations.empty)}
              </CommandEmpty>
              <CommandGroup>
                {filtered.map(({ task, agent }) => {
                  const name = agent?.name ?? t(($) => $.agent_live.fallback_name);
                  return (
                    <CommandItem
                      key={task.id}
                      value={`${name} ${task.id}`}
                      onSelect={() => setSelectedTaskId(task.id)}
                      className="gap-2.5 py-2"
                    >
                      <ActorAvatar
                        actorType="agent"
                        actorId={task.agent_id}
                        size="md"
                        profileLink={false}
                        showStatusDot
                      />
                      <span className="flex min-w-0 flex-1 flex-col">
                        <span className="truncate text-body font-medium">{name}</span>
                        <span className="truncate text-caption text-muted-foreground">
                          {statusLabel(task.status, t)} · {new Date(task.created_at).toLocaleString(locale, {
                            month: "short",
                            day: "numeric",
                            hour: "2-digit",
                            minute: "2-digit",
                          })}
                        </span>
                      </span>
                    </CommandItem>
                  );
                })}
              </CommandGroup>
            </CommandList>
          </Command>
        )}
      </PopoverContent>
    </Popover>
  );
}
