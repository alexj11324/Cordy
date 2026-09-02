/**
 * Centred, tappable title region for the Chat tab's native Stack header.
 * Rendered as `headerTitle: () => <ChatTitleButton ... />` so iOS positions
 * it where it expects the screen title, but the whole region is a Pressable
 * — tap opens the sessions + agent picker sheet.
 */
import { Pressable, View } from "react-native";
import type { Agent, ChatSession } from "@patchbay/core/types";
import { Text } from "@/components/ui/text";
import { ActorAvatar } from "@/components/ui/actor-avatar";
import { chatSessionDisplayTitle } from "@/lib/chat-session-title";
import { useChatCopy } from "@/lib/use-chat-copy";

interface Props {
  currentSession: ChatSession | null;
  currentAgent: Agent | null;
  onPress: () => void;
}

export function ChatTitleButton({
  currentSession,
  currentAgent,
  onPress,
}: Props) {
  const copy = useChatCopy();
  const agentName = currentAgent?.name ?? copy.chat;
  const subtitle = chatSessionDisplayTitle(currentSession?.title, copy.newChat);

  return (
    <Pressable
      onPress={onPress}
      hitSlop={4}
      className="flex-row items-center gap-2 px-2 py-1 rounded-lg active:bg-secondary"
      accessibilityRole="button"
      accessibilityLabel={copy.sessionsAndAgentPicker}
    >
      <ActorAvatar
        type={currentAgent ? "agent" : null}
        id={currentAgent?.id ?? null}
        size={24}
        showPresence
      />
      <View>
        <View className="flex-row items-center gap-1">
          <Text
            className="text-base font-semibold text-foreground"
            numberOfLines={1}
          >
            {agentName}
          </Text>
          <Text className="text-xs text-muted-foreground">▼</Text>
        </View>
        <Text
          className="text-xs text-muted-foreground"
          numberOfLines={1}
        >
          {subtitle}
        </Text>
      </View>
    </Pressable>
  );
}
