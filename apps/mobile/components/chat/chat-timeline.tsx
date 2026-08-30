/**
 * Public per-task event summary — which tools ran and whether a task reported
 * an error. Rendered:
 *
 *   - Live (under the StatusPill while a task is in flight), AND
 *   - Persisted (under the assistant bubble once the message has landed)
 *
 * Public process steps (tool_use / tool_result / error) collapse behind a
 * single "N steps" toggle. Provider thinking payloads are deliberately not
 * rendered: they stay in the task-event cache for status/audit processing but
 * never become user-visible chain-of-thought. Final text is NOT rendered here —
 * the parent renders the assistant message's `content` (or the latest
 * streaming text) as its own markdown block.
 *
 * Folds use RNR `Collapsible` (built on `@rn-primitives/collapsible`).
 * The earlier version of this file hand-rolled four separate
 * `useState + Pressable + chevron` triggers (~60 lines of state +
 * handlers); Collapsible owns open/close + a11y semantics in one place.
 *
 * `defaultOpen` is true on the outer fold while streaming so the user
 * sees activity; the persisted instance below an assistant bubble
 * starts closed (matches web's `OuterProcessFold` behaviour in
 * `packages/views/chat/components/chat-message-list.tsx`).
 */
import { View } from "react-native";
import { Ionicons } from "@expo/vector-icons";
import type { TaskMessagePayload } from "@patchbay/core/types";
import { Text } from "@/components/ui/text";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible";
import {
  formatAgentThreadCopy,
  type AgentThreadCopy,
} from "@/lib/agent-thread-i18n";
import { useAgentThreadCopy } from "@/lib/use-agent-thread-copy";

interface Props {
  items: TaskMessagePayload[];
  /** Whether the owning task is still running. Drives the default-open
   *  state and the dot-pulse next to the trigger. */
  isStreaming?: boolean;
}

export function ChatTimeline({ items, isStreaming = false }: Props) {
  const copy = useAgentThreadCopy();
  const processSteps = items.filter(
    (item) =>
      item.type === "tool_use" ||
      item.type === "tool_result" ||
      item.type === "error",
  );
  if (processSteps.length === 0) return null;
  const processLabel = formatAgentThreadCopy(
    processSteps.length === 1
      ? copy.process_steps_one
      : copy.process_steps_other,
    { count: String(processSteps.length) },
  );

  return (
    <Collapsible defaultOpen={isStreaming}>
      <CollapsibleTrigger asChild>
        <View
          accessibilityRole="button"
          accessibilityLabel={processLabel}
          className="flex-row items-center gap-1 active:opacity-70"
        >
          <Ionicons name="chevron-forward" size={12} color="#71717a" />
          {isStreaming ? <StreamingDot /> : null}
          <Text className="text-xs text-muted-foreground">{processLabel}</Text>
        </View>
      </CollapsibleTrigger>
      <CollapsibleContent>
        <View className="mt-1 rounded-lg border border-border bg-muted/20 px-2 py-1.5 gap-0.5">
          {processSteps.map((item) => (
            <StepRow
              key={`${item.task_id}-${item.seq}`}
              item={item}
              copy={copy}
            />
          ))}
        </View>
      </CollapsibleContent>
    </Collapsible>
  );
}

function StreamingDot() {
  // Single accent dot beside the trigger so the user knows the rows
  // below may still be growing. Real "agent is alive" cue is StatusPill
  // (breathing dots) above; this is a quiet co-signal.
  return <View className="h-1.5 w-1.5 rounded-full bg-primary" />;
}

function StepRow({
  item,
  copy,
}: {
  item: TaskMessagePayload;
  copy: AgentThreadCopy;
}) {
  switch (item.type) {
    case "tool_use":
      return <ToolCallRow item={item} copy={copy} />;
    case "tool_result":
      return <ToolResultRow item={item} copy={copy} />;
    case "error":
      return <ErrorRow item={item} />;
    default:
      return null;
  }
}

function ToolCallRow({
  item,
  copy,
}: {
  item: TaskMessagePayload;
  copy: AgentThreadCopy;
}) {
  return (
    <View
      data-agent-thread-event="tool_use"
      className="py-0.5 flex-row items-center gap-1.5"
    >
      <Ionicons name="construct-outline" size={12} color="#71717a" />
      <Text className="text-xs font-medium text-foreground">
        {item.tool ?? copy.tool_fallback}
      </Text>
    </View>
  );
}

function ToolResultRow({
  item,
  copy,
}: {
  item: TaskMessagePayload;
  copy: AgentThreadCopy;
}) {
  const prefix = item.tool
    ? `${item.tool} ${copy.tool_result_ready}`
    : copy.tool_result_ready;
  return (
    <View
      data-agent-thread-event="tool_result"
      className="py-0.5 flex-row items-center gap-1.5"
    >
      <Ionicons name="checkmark-circle-outline" size={12} color="#71717a" />
      <Text className="text-xs text-muted-foreground">{prefix}</Text>
    </View>
  );
}

function ErrorRow({ item }: { item: TaskMessagePayload }) {
  return (
    <View className="py-0.5 flex-row items-start gap-1.5">
      <Ionicons
        name="alert-circle"
        size={12}
        color="#dc2626"
        style={{ marginTop: 2 }}
      />
      <Text className="flex-1 text-xs text-destructive" numberOfLines={3}>
        {item.content}
      </Text>
    </View>
  );
}
