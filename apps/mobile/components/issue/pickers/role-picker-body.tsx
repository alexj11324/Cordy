import { useMemo } from "react";
import { FlatList, Pressable, View } from "react-native";
import { Ionicons } from "@expo/vector-icons";
import { useColorScheme } from "nativewind";
import { useQuery } from "@tanstack/react-query";
import { Text } from "@/components/ui/text";
import { ActorAvatar } from "@/components/ui/actor-avatar";
import { agentListOptions } from "@/data/queries/agents";
import { memberListOptions } from "@/data/queries/members";
import { teamListOptions } from "@/data/queries/teams";
import { useAuthStore } from "@/data/auth-store";
import { useWorkspaceStore } from "@/data/workspace-store";
import { getIssueRoleCopy } from "@/lib/issue-role-copy";
import {
  buildIssueRoleOptions,
  isIssueRoleOptionSelected,
  type IssueRoleOptionActor,
  type RolePickerKind,
  type RoleValue,
} from "@/lib/issue-role-options";
import { isAgentRuntimeBound } from "@/lib/is-agent-runtime-bound";
import { useScrollToTopOnChange } from "@/lib/use-scroll-to-top-on-change";
import { THEME } from "@/lib/theme";
import { cn } from "@/lib/utils";

const AVATAR_SIZE = 36;

export type { RolePickerKind, RoleValue } from "@/lib/issue-role-options";

export function RolePickerBody({
  kind,
  value,
  query,
  onChange,
  allowUnassigned = true,
  excludedActor = null,
}: {
  kind: RolePickerKind;
  value: RoleValue;
  query: string;
  onChange: (next: RoleValue) => void;
  allowUnassigned?: boolean;
  excludedActor?: RoleValue;
}) {
  const wsId = useWorkspaceStore((state) => state.currentWorkspaceId);
  const language = useAuthStore((state) => state.user?.language);
  const copy = getIssueRoleCopy(language);
  const { data: members = [] } = useQuery(memberListOptions(wsId));
  const { data: agents = [] } = useQuery(agentListOptions(wsId));
  const { data: teams = [] } = useQuery(teamListOptions(wsId));
  const listRef = useScrollToTopOnChange(query);
  const { colorScheme } = useColorScheme();
  const checkColor =
    colorScheme === "dark" ? THEME.dark.primary : THEME.light.primary;
  const rows = useMemo(() => {
    const actorOptions: IssueRoleOptionActor[] = [
      ...members.map((member) => ({
        type: "member" as const,
        id: member.user_id,
        name: member.name,
      })),
      ...agents.map((agent) => ({
        type: "agent" as const,
        id: agent.id,
        name: agent.name,
        archived: Boolean(agent.archived_at),
        needsRuntime: !isAgentRuntimeBound(agent),
      })),
      ...teams.map((team) => ({
        type: "team" as const,
        id: team.id,
        name: team.name,
        archived: Boolean(team.archived_at),
        needsRuntime: !agents.some(
          (agent) => agent.id === team.leader_id && isAgentRuntimeBound(agent),
        ),
      })),
    ];
    return buildIssueRoleOptions({
      kind,
      value,
      query,
      actors: actorOptions,
      allowUnassigned,
      excludedActor,
    });
  }, [
    agents,
    allowUnassigned,
    excludedActor,
    kind,
    members,
    query,
    teams,
    value,
  ]);

  return (
    <FlatList
      ref={listRef}
      data={rows}
      className="flex-1"
      keyboardShouldPersistTaps="handled"
      automaticallyAdjustKeyboardInsets
      contentInsetAdjustmentBehavior="automatic"
      keyExtractor={(row) =>
        row.kind === "unassigned"
          ? "unassigned"
          : `${row.actor.type}:${row.actor.id}`
      }
      renderItem={({ item }) => {
        const needsRuntime =
          item.kind === "actor" && item.actor.needsRuntime === true;
        return (
          <Pressable
            disabled={needsRuntime}
            onPress={() =>
              onChange(
                item.kind === "unassigned"
                  ? null
                  : { type: item.actor.type, id: item.actor.id },
              )
            }
            className={cn(
              "flex-row items-center gap-3 px-4 py-3 active:bg-secondary",
              needsRuntime && "opacity-50",
            )}
          >
            {item.kind === "unassigned" ? (
              <View
                className="items-center justify-center rounded-full border border-dashed border-muted-foreground/40"
                style={{ width: AVATAR_SIZE, height: AVATAR_SIZE }}
              >
                <Text className="text-sm text-muted-foreground">∅</Text>
              </View>
            ) : (
              <ActorAvatar
                type={item.actor.type}
                id={item.actor.id}
                size={AVATAR_SIZE}
              />
            )}
            <Text className="flex-1 text-base text-foreground">
              {item.kind === "unassigned" ? copy.unassigned : item.actor.name}
            </Text>
            {needsRuntime ? (
              <Text className="text-sm text-muted-foreground">
                {item.kind === "actor" && item.actor.type === "team"
                  ? copy.leaderNeedsRuntime
                  : copy.needsRuntime}
              </Text>
            ) : null}
            {isIssueRoleOptionSelected(value, item) ? (
              <Ionicons name="checkmark" size={20} color={checkColor} />
            ) : null}
          </Pressable>
        );
      }}
      ListEmptyComponent={
        <View className="items-center px-3 py-8">
          <Text className="text-sm text-muted-foreground">
            {copy.noMatches}
          </Text>
        </View>
      }
    />
  );
}
