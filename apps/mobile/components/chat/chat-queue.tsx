/**
 * Native queue summary for follow-up turns.
 *
 * The web surface uses AI Elements' Queue primitives. Mobile keeps the same
 * meaning (ordered, server-backed prompts with their current task status)
 * using the shipped React Native Collapsible primitive. There are no local
 * placeholder rows: an empty queue stays hidden and every row comes from the
 * pending-task or Agent-thread response.
 */
import { View } from "react-native";
import { Ionicons } from "@expo/vector-icons";
import type { ChatQueuedTask } from "@orvilo/core/types";
import { Text } from "@/components/ui/text";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible";
import { useChatCopy } from "@/lib/use-chat-copy";
import { useColorScheme } from "@/lib/use-color-scheme";
import { THEME } from "@/lib/theme";

interface Props {
  tasks: readonly ChatQueuedTask[];
}

export function ChatQueue({ tasks }: Props) {
  const copy = useChatCopy();
  if (tasks.length === 0) return null;

  return (
    <Collapsible defaultOpen>
      <CollapsibleTrigger asChild>
        <View
          accessibilityRole="button"
          accessibilityLabel={copy.queueTitle(tasks.length)}
          className="flex-row items-center gap-1.5 px-1 active:opacity-70"
        >
          <Ionicons name="chevron-forward" size={12} color="#71717a" />
          <Ionicons name="list-outline" size={13} color="#71717a" />
          <Text className="text-xs text-muted-foreground">
            {copy.queueTitle(tasks.length)}
          </Text>
        </View>
      </CollapsibleTrigger>
      <CollapsibleContent>
        <View
          accessibilityLiveRegion="polite"
          className="mt-1 rounded-lg border border-border bg-muted/20 px-2 py-1.5 gap-0.5"
        >
          {tasks.map((task) => (
            <QueueRow key={task.task_id} task={task} />
          ))}
        </View>
      </CollapsibleContent>
    </Collapsible>
  );
}

function QueueRow({ task }: { task: ChatQueuedTask }) {
  const copy = useChatCopy();
  const { colorScheme } = useColorScheme();
  const theme = THEME[colorScheme];
  const status = queueStatus(task.status, copy);
  const content = task.content?.trim() || copy.queueFallback;

  return (
    <View
      accessible
      accessibilityLabel={`${content}, ${status.label}`}
      className="min-h-8 flex-row items-center gap-1.5 rounded-md px-1"
    >
      <Ionicons name={status.icon} size={13} color={status.color(theme)} />
      <Text className="flex-1 text-xs text-muted-foreground" numberOfLines={2}>
        {content}
      </Text>
      <Text className="text-[10px] text-muted-foreground/80">
        {status.label}
      </Text>
    </View>
  );
}

type Theme = typeof THEME.light;
type QueueStatus = {
  label: string;
  icon: keyof typeof Ionicons.glyphMap;
  color: (theme: Theme) => string;
};

function queueStatus(status: string, copy: ReturnType<typeof useChatCopy>): QueueStatus {
  switch (status) {
    case "deferred":
      return {
        label: copy.status.retrying,
        icon: "refresh-outline",
        color: (theme) => theme.warning,
      };
    case "dispatched":
      return {
        label: copy.status.startingUp,
        icon: "play-circle-outline",
        color: (theme) => theme.info,
      };
    case "running":
      return {
        label: copy.status.working,
        icon: "time-outline",
        color: (theme) => theme.info,
      };
    case "waiting_local_directory":
      return {
        label: copy.status.queued,
        icon: "pause-circle-outline",
        color: (theme) => theme.warning,
      };
    case "queued":
    default:
      return {
        label: copy.status.queued,
        icon: "ellipse-outline",
        color: (theme) => theme.mutedForeground,
      };
  }
}
