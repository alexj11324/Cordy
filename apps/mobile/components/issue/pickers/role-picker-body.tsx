/**
 * Native form-sheet picker for the three Issue roles.
 *
 * Owner is restricted to active workspace members. Reviewer accepts members,
 * agents, and teams; runnable agent/team rows are disabled when their runtime
 * is not bound. The route owns mutation/draft persistence, keeping this body
 * a pure list just like the executor picker.
 */
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
import { useWorkspaceStore } from "@/data/workspace-store";
import { useScrollToTopOnChange } from "@/lib/use-scroll-to-top-on-change";
import { THEME } from "@/lib/theme";
import { cn } from "@/lib/utils";
import { isAgentRuntimeBound } from "@/lib/is-agent-runtime-bound";
import { useAgentThreadCopy } from "@/lib/use-agent-thread-copy";

const AVATAR_SIZE = 36;

export type RoleValue = { type: IssueActorType; id: string } | null;
export type RolePickerKind = "owner" | "reviewer";

type Props = {
  kind: RolePickerKind;
  value: RoleValue;
  query: string;
  onChange: (next: RoleValue) => void;
};

type Row =
  | { kind: "unassigned" }
  | { kind: "member"; member: MemberWithUser }
  | { kind: "agent"; agent: Agent }
  | { kind: "team"; team: Team };

function selected(value: RoleValue, row: Row): boolean {
  if (row.kind === "unassigned") return value === null;
  const id =
    row.kind === "member"
      ? row.member.user_id
      : row.kind === "agent"
        ? row.agent.id
        : row.team.id;
  return value?.type === row.kind && value.id === id;
}

export function RolePickerBody({ kind, value, query, onChange }: Props) {
  const wsId = useWorkspaceStore((s) => s.currentWorkspaceId);
  const { data: members = [] } = useQuery(memberListOptions(wsId));
  const { data: agents = [] } = useQuery(agentListOptions(wsId));
  const { data: teams = [] } = useQuery(teamListOptions(wsId));
  const listRef = useScrollToTopOnChange(query);
  const { colorScheme } = useColorScheme();
  const checkColor = colorScheme === "dark" ? THEME.dark.primary : THEME.light.primary;
  const copy = useAgentThreadCopy();

  const rows = useMemo<Row[]>(() => {
    const q = query.trim().toLowerCase();
    const matches = (name: string) => !q || name.toLowerCase().includes(q);
    const memberRows: Row[] =
      kind === "owner" || kind === "reviewer"
        ? members
            .filter((member) => matches(member.name))
            .sort((a, b) => a.name.localeCompare(b.name))
            .map((member) => ({ kind: "member", member }))
        : [];
    const agentRows: Row[] =
      kind === "reviewer"
        ? agents
            .filter((agent) => !agent.archived_at && matches(agent.name))
            .sort((a, b) => a.name.localeCompare(b.name))
            .map((agent) => ({ kind: "agent", agent }))
        : [];
    const teamRows: Row[] =
      kind === "reviewer"
        ? teams
            .filter((team) => !team.archived_at && matches(team.name))
            .sort((a, b) => a.name.localeCompare(b.name))
            .map((team) => ({ kind: "team", team }))
        : [];
    const all = [...memberRows, ...agentRows, ...teamRows];
    if (q) return all;
    const current = all.find((row) => selected(value, row));
    return [{ kind: "unassigned" }, ...(current ? [current] : []), ...all.filter((row) => !selected(value, row))];
  }, [agents, kind, members, query, teams, value]);

  return (
    <FlatList
      ref={listRef}
      data={rows}
      className="flex-1"
      keyboardShouldPersistTaps="handled"
      automaticallyAdjustKeyboardInsets
      contentInsetAdjustmentBehavior="automatic"
      keyExtractor={(row) => {
        if (row.kind === "unassigned") return "unassigned";
        return `${row.kind}:${row[row.kind].id}`;
      }}
      renderItem={({ item }) => {
        const needsRuntime =
          (item.kind === "agent" && !isAgentRuntimeBound(item.agent)) ||
          (item.kind === "team" && !agents.some((agent) => agent.id === item.team.leader_id && isAgentRuntimeBound(agent)));
        const disabled = needsRuntime;
        return (
          <Pressable
            disabled={disabled}
            onPress={() => {
              onChange(item.kind === "unassigned" ? null : { type: item.kind, id: item[item.kind].id });
            }}
            className={cn("flex-row items-center gap-3 px-4 py-3 active:bg-secondary", disabled && "opacity-50")}
          >
            {item.kind === "unassigned" ? (
              <View className="rounded-full border border-dashed border-muted-foreground/40 items-center justify-center" style={{ width: AVATAR_SIZE, height: AVATAR_SIZE }}>
                <Text className="text-sm text-muted-foreground">∅</Text>
              </View>
            ) : (
              <ActorAvatar type={item.kind} id={item[item.kind].id} size={AVATAR_SIZE} />
            )}
            <Text className="flex-1 text-base text-foreground">
              {item.kind === "unassigned" ? copy.role_picker_unassigned : item[item.kind].name}
            </Text>
            {needsRuntime ? <Text className="text-sm text-muted-foreground">{copy.role_picker_needs_runtime}</Text> : null}
            {selected(value, item) ? <Ionicons name="checkmark" size={20} color={checkColor} /> : null}
          </Pressable>
        );
      }}
      ListEmptyComponent={
        <View className="px-3 py-8 items-center">
          <Text className="text-sm text-muted-foreground">{copy.role_picker_no_matches}</Text>
        </View>
      }
    />
  );
}
