import { Ionicons } from "@expo/vector-icons";
import { View } from "react-native";
import { Text } from "@/components/ui/text";
import { useChatCopy } from "@/lib/use-chat-copy";

export function RuntimeRequiredBanner({ agentName }: { agentName?: string }) {
  const copy = useChatCopy();
  const name = agentName?.trim() || copy.runtimeFallbackName;
  return (
    <View className="mx-3 mb-1.5 flex-row items-center gap-1.5 rounded-md bg-warning/15 px-2.5 py-1.5">
      <Ionicons name="server-outline" size={14} color="#a16207" />
      <Text className="flex-1 text-xs text-warning">
        {copy.runtimeRequiredBanner(name)}
      </Text>
    </View>
  );
}
