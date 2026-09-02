/**
 * Empty-state surface shown when the active session has no messages.
 *
 * Two modes mirror web (packages/views/chat/components/chat-window.tsx
 * `EmptyState`):
 *
 *   - first-time (the workspace has never started a chat) → educate and
 *     offer conversation starters so the composer is not a blank dead end.
 *   - returning (at least one prior session exists) → lead with starter
 *     starters. Tapping prefills the draft so the user can edit before sending.
 *
 * Copy uses the mobile chat adapter, whose four locales mirror the web
 * `chat.json` namespace for this surface. Agent-authored starters remain
 * server-provided and are displayed verbatim.
 */
import { View } from "react-native";
import type { Agent } from "@patchbay/core/types";
import { Text } from "@/components/ui/text";
import { Button } from "@/components/ui/button";
import { useChatCopy } from "@/lib/use-chat-copy";

interface Props {
  hasSessions: boolean;
  agent: Agent | null;
  onPickPrompt: (text: string) => void;
}

export function ChatEmptyState({ hasSessions, agent, onPickPrompt }: Props) {
  const copy = useChatCopy();
  const title = copy.emptyTitle(agent?.name ?? null);
  const configured = (agent?.conversation_starters ?? []).filter(
    (item) => item.label.trim() && item.prompt.trim(),
  );
  const starters = configured.length > 0 ? configured : copy.fallbackStarters;
  return (
    <View className="flex-1 items-center justify-center px-6 py-8 gap-5">
      <View className="items-center gap-1">
        <Text className="text-base font-semibold text-foreground text-center">
          {title}
        </Text>
        {agent?.description ? (
          <Text className="text-sm text-muted-foreground text-center">
            {agent.description}
          </Text>
        ) : null}
        {!hasSessions ? (
          <Text className="text-sm text-muted-foreground text-center">
            {copy.emptyFirstTimeHint}
          </Text>
        ) : null}
      </View>
      {agent ? (
        <View className="w-full max-w-xs gap-2">
          {starters.map((item, index) => (
            <Button
              key={index}
              variant="outline"
              onPress={() => onPickPrompt(item.prompt)}
              className="h-auto justify-start px-3 py-2.5"
              accessibilityLabel={item.label}
            >
              <Text className="text-sm text-foreground">{item.label}</Text>
            </Button>
          ))}
        </View>
      ) : null}
    </View>
  );
}
