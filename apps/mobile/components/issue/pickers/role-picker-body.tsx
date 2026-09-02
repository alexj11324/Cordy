import { useMemo } from "react";
import { FlatList, Pressable, View } from "react-native";
import { Ionicons } from "@expo/vector-icons";
import { useColorScheme } from "nativewind";
import { useQuery } from "@tanstack/react-query";
import type {
  Agent,
  IssueActorType,
  MemberWithUser,
  Team,
} from "@patchbay/core/types";
import { Text } from "@/components/ui/text";
import { ActorAvatar } from "@/components/ui/actor-avatar";
import { agentListOptions } from "@/data/queries/agents";
import { memberListOptions } from "@/data/queries/members";
import { teamListOptions } from "@/data/queries/teams";
import { useAuthStore } from "@/data/auth-store";
import { useWorkspaceStore } from "@/data/workspace-store";
import { getIssueRoleCopy } from "@/lib/issue-role-copy";
import { isAgentRuntimeBound } from "@/lib/is-agent-runtime-bound";
import { useScrollToTopOnChange } from "@/lib/use-scroll-to-top-on-change";
import { THEME } from "@/lib/theme";
import { cn } from "@/lib/utils";

const AVATAR_SIZE = 36;

export type RoleValue = { type: IssueActorType; id: string } | null;
export type RolePickerKind = "owner" | "reviewer";

type Row =
  | { kind: "unassigned" }
  | { kind: "member"; member: MemberWithUser }
  | { kind: "agent"; agent: Agent }
  | { kind: "team"; team: Team };

function rowId(row: Exclude<Row, { kind: "unassigned" }>): string {
  if (row.kind === "member") return row.member.user_id;
  return row.kind === "agent" ? row.agent.id : row.team.id;
}

function isSelected(value: RoleValue, row: Row): boolean {
  if (row.kind === "unassigned") return value === null;
  return value?.type === row.kind && value.id === rowId(row);
}

export function RolePickerBody({
  kind,
  value,
  query,
  onChange,
}: {
  kind: RolePickerKind;
  value: RoleValue;
  query: string;
  onChange: (next: RoleValue) => void;
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
  const rows = useMemo<Row[]>(() => {
    const needle = query.trim().toLowerCase();
    const matches = (name: string) =>
      !needle || name.toLowerCase().includes(needle);
    const memberRows: Row[] = members
      .filter((member) => matches(member.name))
      .sort((left, right) => left.name.localeCompare(right.name))
      .map((member) => ({ kind: "member", member }));
    const agentRows: Row[] =
      kind === "reviewer"
        ? agents
            .filter((agent) => !agent.archived_at && matches(agent.name))
            .sort((left, right) => left.name.localeCompare(right.name))
            .map((agent) => ({ kind: "agent", agent }))
        : [];
    const teamRows: Row[] =
      kind === "reviewer"
        ? teams
            .filter((team) => !team.archived_at && matches(team.name))
            .sort((left, right) => left.name.localeCompare(right.name))
            .map((team) => ({ kind: "team", team }))
        : [];
    const actors = [...memberRows, ...agentRows, ...teamRows];
    if (needle) return actors;
    const current = actors.find((row) => isSelected(value, row));
    const allowUnassigned = kind !== "reviewer" || value === null;
    return [
      ...(allowUnassigned ? [{ kind: "unassigned" as const }] : []),
      ...(current ? [current] : []),
      ...actors.filter((row) => !isSelected(value, row)),
    ];
  }, [agents, kind, members, query, teams, value]);

  return (
    <FlatList
      ref={listRef}
      data={rows}
      className="flex-1"
      keyboardShouldPersistTaps="handled"
      automaticallyAdjustKeyboardInsets
      contentInsetAdjustmentBehavior="automatic"
      keyExtractor={(row) =>
        row.kind === "unassigned" ? "unassigned" : `${row.kind}:${rowId(row)}`
      }
      renderItem={({ item }) => {
        const needsRuntime =
          (item.kind === "agent" && !isAgentRuntimeBound(item.agent)) ||
          (item.kind === "team" &&
            !agents.some(
              (agent) =>
                agent.id === item.team.leader_id && isAgentRuntimeBound(agent),
            ));
        return (
          <Pressable
            disabled={needsRuntime}
            onPress={() =>
              onChange(
                item.kind === "unassigned"
                  ? null
                  : { type: item.kind, id: rowId(item) },
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
                id={rowId(item)}
                size={AVATAR_SIZE}
              />
            )}
            <Text className="flex-1 text-base text-foreground">
              {item.kind === "unassigned"
                ? copy.unassigned
                : item.kind === "member"
                  ? item.member.name
                  : item.kind === "agent"
                    ? item.agent.name
                    : item.team.name}
            </Text>
            {needsRuntime ? (
              <Text className="text-sm text-muted-foreground">
                {item.kind === "team"
                  ? copy.leaderNeedsRuntime
                  : copy.needsRuntime}
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
