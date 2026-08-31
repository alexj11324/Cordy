/**
 * Single row inside the agent-runs formSheet route
 * (`app/(app)/[workspace]/issue/[id]/runs.tsx`). Same component for active
 * and past tasks —
 * the trailing Cancel button is conditional on an active status, and the
 * status badge / colour swaps based on the
 * AgentTask.status enum.
 *
 * Every row opens the same interactive Agent thread route. A terminal task
 * remains conversational when the provider session is available, while the
 * server-provided unavailable reason disables continuation explicitly.
 */
import { Alert, Pressable, View } from "react-native";
import { isAgentTaskActive } from "@patchbay/core/agent-thread";
import type { AgentTask } from "@patchbay/core/types";
import { Text } from "@/components/ui/text";
import { ActorAvatar } from "@/components/ui/actor-avatar";
import { useCancelTask } from "@/data/mutations/issues";
import { useActorLookup } from "@/data/use-actor-name";
import { timeAgo } from "@/lib/time-ago";
import {
  formatAgentThreadCopy,
  type AgentThreadCopy,
} from "@/lib/agent-thread-i18n";
import { useAgentThreadCopy } from "@/lib/use-agent-thread-copy";

interface Props {
  task: AgentTask;
  issueId: string;
  onOpen: () => void;
}

export function RunRow({ task, issueId, onOpen }: Props) {
  const { getName } = useActorLookup();
  const copy = useAgentThreadCopy();
  const isActive = isAgentTaskActive(task);
  const summary = task.trigger_summary?.trim() || fallbackSummary(task, copy);
  // Past tasks use completed_at when present (server fills it for terminal
  // statuses); active tasks fall back to created_at so the user sees how
  // long it's been waiting.
  const timestamp = task.completed_at || task.created_at;

  return (
    <Pressable
      onPress={onOpen}
      accessibilityRole="button"
      accessibilityLabel={formatAgentThreadCopy(copy.open_thread_for, {
        summary,
      })}
      className="flex-row items-start gap-3 py-2 active:opacity-70"
    >
      <ActorAvatar type="agent" id={task.agent_id} size={28} showPresence />
      <View className="flex-1 gap-1">
        <Text className="text-sm text-foreground" numberOfLines={2}>
          <Text className="font-medium">{getName("agent", task.agent_id)}</Text>
          <Text className="text-muted-foreground"> · {summary}</Text>
        </Text>
        <View className="flex-row items-center gap-2">
          <StatusBadge task={task} copy={copy} />
          <Text className="text-xs text-muted-foreground">
            {timestamp ? timeAgo(timestamp) : ""}
          </Text>
        </View>
      </View>
      {isActive ? (
        <CancelButton taskId={task.id} issueId={issueId} copy={copy} />
      ) : null}
    </Pressable>
  );
}

function StatusBadge({
  task,
  copy,
}: {
  task: AgentTask;
  copy: AgentThreadCopy;
}) {
  const label =
    copy.status[task.status as keyof AgentThreadCopy["status"]] ?? task.status;
  const cls = STATUS_CLASS[task.status] ?? "text-muted-foreground";
  // For failed tasks, surface the failure_reason inline so users don't have
  // to drill in. Missing / empty / unrecognised stays as just "Failed".
  if (task.status === "failed" && task.failure_reason) {
    const reasonLabel =
      copy.failure[task.failure_reason as keyof AgentThreadCopy["failure"]];
    if (reasonLabel) {
      return (
        <Text className={`text-xs ${cls}`}>
          {label} · {reasonLabel}
        </Text>
      );
    }
  }
  return <Text className={`text-xs ${cls}`}>{label}</Text>;
}

function CancelButton({
  taskId,
  issueId,
  copy,
}: {
  taskId: string;
  issueId: string;
  copy: AgentThreadCopy;
}) {
  const mutation = useCancelTask(issueId);

  const onPress = () => {
    Alert.alert(copy.cancel_task_title, copy.cancel_task_body, [
      { text: copy.keep_running, style: "cancel" },
      {
        text: copy.cancel_task,
        style: "destructive",
        onPress: () => mutation.mutate(taskId),
      },
    ]);
  };

  return (
    <Pressable
      onPress={(event) => {
        event.stopPropagation();
        onPress();
      }}
      disabled={mutation.isPending}
      className="px-3 py-1.5 rounded-md bg-secondary active:opacity-70"
    >
      <Text className="text-xs font-medium text-foreground">{copy.cancel}</Text>
    </Pressable>
  );
}

function fallbackSummary(task: AgentTask, copy: AgentThreadCopy): string {
  switch (task.kind) {
    case "comment":
      return copy.comment_task;
    case "automation":
      return copy.automation_run;
    case "chat":
      return copy.chat_task;
    case "quick_create":
      return copy.quick_create;
    case "direct":
    default:
      return copy.task;
  }
}

const STATUS_CLASS: Record<string, string> = {
  queued: "text-muted-foreground",
  deferred: "text-muted-foreground",
  dispatched: "text-brand",
  waiting_local_directory: "text-muted-foreground",
  waiting_capacity: "text-muted-foreground",
  running: "text-brand",
  completed: "text-muted-foreground",
  failed: "text-destructive",
  cancelled: "text-muted-foreground",
};
