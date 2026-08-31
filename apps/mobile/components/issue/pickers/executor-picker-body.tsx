/**
 * Pure picker body for issue executor — polymorphic single-select over
 * agents + teams, plus an "Unassigned" option. Human responsibility is
 * edited through the separate owner picker. See
 * status-picker-body.tsx for the split rationale.
 *
 * Mirrors web `packages/views/issues/components/pickers/executor-picker.tsx`
 * (mobile skips frequency-sort; alphabetical instead).
 *
 * Header + search bar are owned by the iOS native nav header registered in
 * `app/(app)/[workspace]/_layout.tsx` (executor Stack.Screen sets
 * `headerShown: true` + `title`); the route file wires
 * `headerSearchBarOptions.onChangeText` to a local `query` state and passes
 * it in as the `query` prop. This body is just a FlatList — no chrome.
 */
import { useMemo } from "react";
import { FlatList, Pressable, View } from "react-native";
import { useQuery } from "@tanstack/react-query";
import { Ionicons } from "@expo/vector-icons";
import { useColorScheme } from "nativewind";
import type {
  Agent,
  IssueExecutorType,
  Team,
} from "@patchbay/core/types";
import { Text } from "@/components/ui/text";
import { ActorAvatar } from "@/components/ui/actor-avatar";
import { agentListOptions } from "@/data/queries/agents";
import { teamListOptions } from "@/data/queries/teams";
import { useWorkspaceStore } from "@/data/workspace-store";
import { useScrollToTopOnChange } from "@/lib/use-scroll-to-top-on-change";
import { THEME } from "@/lib/theme";
import { cn } from "@/lib/utils";
import { isAgentRuntimeBound } from "@/lib/is-agent-runtime-bound";

const AVATAR_SIZE = 36;

export type ExecutorValue = {
  type: IssueExecutorType;
  id: string;
} | null;

interface Props {
  value: ExecutorValue;
  query: string;
  onChange: (next: ExecutorValue) => void;
}

type Row =
  | { kind: "unassigned" }
  | { kind: "agent"; agent: Agent }
  | { kind: "team"; team: Team };

function isRowSelected(value: ExecutorValue, row: Row): boolean {
  if (row.kind === "unassigned") return value === null;
  if (value === null) return false;
  if (row.kind === "agent")
    return value.type === "agent" && value.id === row.agent.id;
  return value.type === "team" && value.id === row.team.id;
}

export function ExecutorPickerBody({ value, query, onChange }: Props) {
  const wsId = useWorkspaceStore((s) => s.currentWorkspaceId);
  const { data: agents = [] } = useQuery(agentListOptions(wsId));
  const { data: teams = [] } = useQuery(teamListOptions(wsId));
  const runnableAgentIds = useMemo(
    () =>
      new Set(
        agents
          .filter((agent) => !agent.archived_at && isAgentRuntimeBound(agent))
          .map((agent) => agent.id),
      ),
    [agents],
  );
  const listRef = useScrollToTopOnChange(query);
  const { colorScheme } = useColorScheme();
  // Tint color for the checkmark accessory. Project uses a monochrome
  // shadcn palette where `primary` is the canonical tint (near-black light /
  // near-white dark); matches Apple HIG's "tintColor" semantics for
  // selection accessories.
  const checkColor =
    colorScheme === "dark" ? THEME.dark.primary : THEME.light.primary;

  const rows = useMemo<Row[]>(() => {
    const q = query.trim().toLowerCase();
    const matchName = (name: string) => !q || name.toLowerCase().includes(q);

    const agentRows: Row[] = [...agents]
      .filter((a) => matchName(a.name))
      .sort((a, b) => a.name.localeCompare(b.name))
      .map((a) => ({ kind: "agent" as const, agent: a }));
    const teamRows: Row[] = [...teams]
      .filter((s) => !s.archived_at && matchName(s.name))
      .sort((a, b) => a.name.localeCompare(b.name))
      .map((s) => ({ kind: "team" as const, team: s }));

    if (q) return [...agentRows, ...teamRows];

    // Pin the currently-selected actor right below Unassigned and remove it
    // from its own section so it doesn't render twice. Apple HIG doesn't
    // require this — it's a product UX choice that speeds up the common
    // "see who's assigned + reassign nearby" path. Skipped when query is
    // active because search-result order should reflect matches, not state.
    const all = [...agentRows, ...teamRows];
    const selectedRow = all.find((r) => isRowSelected(value, r));
    return [
      { kind: "unassigned" },
      ...(selectedRow ? [selectedRow] : []),
      ...agentRows.filter((r) => !isRowSelected(value, r)),
      ...teamRows.filter((r) => !isRowSelected(value, r)),
    ];
  }, [agents, teams, query, value]);

  const isSelected = (row: Row) => isRowSelected(value, row);

  const select = (row: Row) => {
    if (row.kind === "unassigned") onChange(null);
    else if (row.kind === "agent")
      onChange({ type: "agent", id: row.agent.id });
    else onChange({ type: "team", id: row.team.id });
  };

  // FlatList is returned as the route's direct child so RNSScreenContentWrapper
  // can find it as a direct subview and apply the iOS formSheet header offset.
  // See react-native-screens#3634 — wrapping in a parent <View> hides the list
  // from the native search and the rows render at y=0, overlapping the header.
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
        if (row.kind === "agent") return `a:${row.agent.id}`;
        return `s:${row.team.id}`;
      }}
      renderItem={({ item }) => {
        const needsRuntime =
          (item.kind === "agent" && !isAgentRuntimeBound(item.agent)) ||
          (item.kind === "team" &&
            !runnableAgentIds.has(item.team.leader_id));
        return (
          <Pressable
          disabled={needsRuntime}
          onPress={() => select(item)}
          className={cn(
            "flex-row items-center gap-3 px-4 py-3 active:bg-secondary",
            needsRuntime && "opacity-50",
          )}
        >
          {item.kind === "unassigned" ? (
            <View
              className="rounded-full border border-dashed border-muted-foreground/40 items-center justify-center"
              style={{ width: AVATAR_SIZE, height: AVATAR_SIZE }}
            >
              <Text className="text-sm text-muted-foreground">∅</Text>
            </View>
          ) : item.kind === "agent" ? (
            <ActorAvatar type="agent" id={item.agent.id} size={AVATAR_SIZE} />
          ) : (
            <ActorAvatar type="team" id={item.team.id} size={AVATAR_SIZE} />
          )}
          <Text className="flex-1 text-base text-foreground">
            {item.kind === "unassigned"
              ? "Unassigned"
              : item.kind === "agent"
                  ? item.agent.name
                  : item.team.name}
          </Text>
          {/* Right-aligned secondary label. Mirrors Apple's
              UITableViewCellStyleValue1 / UIListContentConfiguration.valueCell
              pattern used throughout iOS Settings — type tag in lighter font on
              the same row. Members carry no tag (they're the default actor). */}
          {item.kind === "agent" ? (
            <Text className="text-sm text-muted-foreground">
              {isAgentRuntimeBound(item.agent) ? "Agent" : "Needs runtime"}
            </Text>
          ) : item.kind === "team" ? (
            <Text className="text-sm text-muted-foreground">
              {needsRuntime ? "Leader needs runtime" : "Team"}
            </Text>
          ) : null}
          {isSelected(item) ? (
            <Ionicons name="checkmark" size={20} color={checkColor} />
          ) : null}
          </Pressable>
        );
      }}
      ListEmptyComponent={
        <View className="px-3 py-8 items-center">
          <Text className="text-sm text-muted-foreground">No matches.</Text>
        </View>
      }
    />
  );
}
