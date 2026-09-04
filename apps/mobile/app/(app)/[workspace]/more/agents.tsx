/**
 * Read-only native Agent entry point for direct chat. Agent management stays
 * on web/desktop; this screen only lists agents the current member can invoke
 * and forwards the selected id to the existing ChatTab `?agent=` intent.
 */
import { useMemo } from "react";
import { ActivityIndicator, Pressable, ScrollView, View } from "react-native";
import { Ionicons } from "@expo/vector-icons";
import { router, Stack } from "expo-router";
import { useQuery } from "@tanstack/react-query";
import type { Agent } from "@patchbay/core/types";
import { canAssignAgentToIssue } from "@patchbay/core/permissions";
import { Text } from "@/components/ui/text";
import { ActorAvatar } from "@/components/ui/actor-avatar";
import { agentListOptions } from "@/data/queries/agents";
import { memberListOptions } from "@/data/queries/members";
import { useAuthStore } from "@/data/auth-store";
import { useWorkspaceStore } from "@/data/workspace-store";
import type { ChatCopy } from "@/lib/chat-copy";
import { chatAgentHref } from "@/lib/chat-session-state";
import { isAgentRuntimeBound } from "@/lib/is-agent-runtime-bound";
import { useChatCopy } from "@/lib/use-chat-copy";

export default function AgentsPage() {
  const copy = useChatCopy();
  const wsId = useWorkspaceStore((state) => state.currentWorkspaceId);
  const wsSlug = useWorkspaceStore((state) => state.currentWorkspaceSlug);
  const userId = useAuthStore((state) => state.user?.id);
  const { data: agents = [], isLoading: agentsLoading } = useQuery(
    agentListOptions(wsId),
  );
  const { data: members = [], isLoading: membersLoading } = useQuery(
    memberListOptions(wsId),
  );
  const memberRole = useMemo(
    () => members.find((member) => member.user_id === userId)?.role ?? null,
    [members, userId],
  );
  const availableAgents = useMemo(
    () =>
      agents.filter(
        (agent) =>
          !agent.archived_at &&
          canAssignAgentToIssue(agent, {
            userId: userId ?? null,
            role: memberRole,
          }).allowed,
      ),
    [agents, memberRole, userId],
  );

  const openChat = (agent: Agent) => {
    if (!wsSlug) return;
    router.replace(chatAgentHref(wsSlug, agent.id));
  };

  return (
    <>
      <Stack.Screen
        options={{ title: copy.agents, headerBackTitle: copy.back }}
      />
      <View className="flex-1 bg-background">
        {agentsLoading || membersLoading ? (
          <View className="flex-1 items-center justify-center">
            <ActivityIndicator />
          </View>
        ) : availableAgents.length === 0 ? (
          <View className="flex-1 items-center justify-center px-6">
            <Text className="text-sm text-muted-foreground text-center">
              {copy.noAgentsAvailable}
            </Text>
          </View>
        ) : (
          <ScrollView
            className="flex-1"
            contentContainerClassName="gap-2 px-4 py-4"
            showsVerticalScrollIndicator={false}
          >
            {availableAgents.map((agent) => (
              <AgentChatRow
                key={agent.id}
                agent={agent}
                copy={copy}
                onPress={() => openChat(agent)}
              />
            ))}
          </ScrollView>
        )}
      </View>
    </>
  );
}

function AgentChatRow({
  agent,
  copy,
  onPress,
}: {
  agent: Agent;
  copy: ChatCopy;
  onPress: () => void;
}) {
  const name = agent.name.trim() || copy.runtimeFallbackName;
  const runtimeBound = isAgentRuntimeBound(agent);

  return (
    <Pressable
      onPress={onPress}
      accessibilityRole="button"
      accessibilityLabel={copy.openChatWith(name)}
      className="flex-row items-center gap-3 rounded-xl border border-border bg-card px-3 py-3 active:bg-secondary"
    >
      <ActorAvatar type="agent" id={agent.id} size={40} showPresence />
      <View className="min-w-0 flex-1 gap-0.5">
        <Text
          className="text-sm font-semibold text-foreground"
          numberOfLines={1}
        >
          {name}
        </Text>
        {agent.description.trim() ? (
          <Text className="text-xs text-muted-foreground" numberOfLines={2}>
            {agent.description}
          </Text>
        ) : null}
        {!runtimeBound ? (
          <Text className="text-xs font-medium text-warning">
            {copy.needsRuntime}
          </Text>
        ) : null}
      </View>
      <Ionicons
        name="chevron-forward"
        size={18}
        color="#71717a"
        accessible={false}
      />
    </Pressable>
  );
}
