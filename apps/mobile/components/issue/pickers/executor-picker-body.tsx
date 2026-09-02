import { useMemo } from "react";
import { FlatList, Pressable, View } from "react-native";
import { Ionicons } from "@expo/vector-icons";
import { useColorScheme } from "nativewind";
import { useQuery } from "@tanstack/react-query";
import type { Agent, IssueExecutorType, Team } from "@patchbay/core/types";
import { Text } from "@/components/ui/text";
import { ActorAvatar } from "@/components/ui/actor-avatar";
import { agentListOptions } from "@/data/queries/agents";
import { teamListOptions } from "@/data/queries/teams";
import { useAuthStore } from "@/data/auth-store";
import { useWorkspaceStore } from "@/data/workspace-store";
import { useScrollToTopOnChange } from "@/lib/use-scroll-to-top-on-change";
import { getIssueRoleCopy } from "@/lib/issue-role-copy";
import { isAgentRuntimeBound } from "@/lib/is-agent-runtime-bound";
import { THEME } from "@/lib/theme";
import { cn } from "@/lib/utils";

const AVATAR_SIZE = 36;

export type ExecutorValue = { type: IssueExecutorType; id: string } | null;

type Row =
  | { kind: "unassigned" }
  | { kind: "agent"; agent: Agent }
  | { kind: "team"; team: Team };

function isSelected(value: ExecutorValue, row: Row): boolean {
  if (row.kind === "unassigned") return value === null;
  if (!value) return false;
  return row.kind === "agent"
    ? value.type === "agent" && value.id === row.agent.id
    : value.type === "team" && value.id === row.team.id;
}

export function ExecutorPickerBody({
  value,
  query,
  onChange,
}: {
  value: ExecutorValue;
  query: string;
  onChange: (next: ExecutorValue) => void;
}) {
  const wsId = useWorkspaceStore((state) => state.currentWorkspaceId);
  const language = useAuthStore((state) => state.user?.language);
  const copy = getIssueRoleCopy(language);
  const { data: agents = [] } = useQuery(agentListOptions(wsId));
  const { data: teams = [] } = useQuery(teamListOptions(wsId));
  const listRef = useScrollToTopOnChange(query);
  const { colorScheme } = useColorScheme();
  const checkColor =
    colorScheme === "dark" ? THEME.dark.primary : THEME.light.primary;
  const runnableAgentIds = useMemo(
    () =>
      new Set(
        agents
          .filter((agent) => !agent.archived_at && isAgentRuntimeBound(agent))
          .map((agent) => agent.id),
      ),
    [agents],
  );
  const rows = useMemo<Row[]>(() => {
    const needle = query.trim().toLowerCase();
    const matches = (name: string) =>
      !needle || name.toLowerCase().includes(needle);
    const agentRows: Row[] = agents
      .filter((agent) => !agent.archived_at && matches(agent.name))
      .sort((left, right) => left.name.localeCompare(right.name))
      .map((agent) => ({ kind: "agent", agent }));
    const teamRows: Row[] = teams
      .filter((team) => !team.archived_at && matches(team.name))
      .sort((left, right) => left.name.localeCompare(right.name))
      .map((team) => ({ kind: "team", team }));
    const actors = [...agentRows, ...teamRows];
    if (needle) return actors;
    const current = actors.find((row) => isSelected(value, row));
    return [
      { kind: "unassigned" },
      ...(current ? [current] : []),
      ...actors.filter((row) => !isSelected(value, row)),
    ];
  }, [agents, query, teams, value]);

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
          : `${row.kind}:${row.kind === "agent" ? row.agent.id : row.team.id}`
      }
      renderItem={({ item }) => {
        const needsRuntime =
          (item.kind === "agent" && !isAgentRuntimeBound(item.agent)) ||
          (item.kind === "team" &&
            !runnableAgentIds.has(item.team.leader_id));
        const id =
          item.kind === "agent"
            ? item.agent.id
            : item.kind === "team"
              ? item.team.id
              : null;
        return (
          <Pressable
            disabled={needsRuntime}
            onPress={() =>
              onChange(
                item.kind === "unassigned"
                  ? null
                  : { type: item.kind, id: id! },
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
                type={item.kind}
                id={id!}
                size={AVATAR_SIZE}
              />
            )}
            <Text className="flex-1 text-base text-foreground">
              {item.kind === "unassigned"
                ? copy.unassigned
                : item.kind === "agent"
                  ? item.agent.name
                  : item.team.name}
            </Text>
            {item.kind === "agent" ? (
              <Text className="text-sm text-muted-foreground">
                {needsRuntime ? copy.needsRuntime : copy.agent}
              </Text>
            ) : item.kind === "team" ? (
              <Text className="text-sm text-muted-foreground">
                {needsRuntime ? copy.leaderNeedsRuntime : copy.team}
              </Text>
            ) : null}
            {isSelected(value, item) ? (
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
