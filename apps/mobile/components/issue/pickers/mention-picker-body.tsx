/**
 * Pure picker body for the comment composer's @mention chips.
 *
 * Mirrors `LabelPickerBody`: multi-select with tap-to-toggle, sheet stays
 * open across toggles, user dismisses via grabber drag-down or Back. The
 * composer's chip row reflects the store live (sheet is presented over
 * the composer; the row is partly visible behind the sheet).
 *
 * Sections (alphabetical within each):
 *   1. `@all` (pinned top, filtered by query)
 *   2. People
 *   3. Agents
 *   4. Teams (archived hidden)
 *   5. Issues (server-side `api.searchIssues`, debounced 200ms; empty
 *      query → no issues section, matching web's mention-suggestion.tsx)
 *
 * Mobile is the iOS-native equivalent of shadcn's `CommandDialog` — search
 * input from the native UISearchController (registered by the parent
 * route via `useNativeSearchBar`), groups via uppercase section labels,
 * empty state inline.
 */
import { useEffect, useMemo, useState } from "react";
import { FlatList, Pressable, View } from "react-native";
import { useQuery } from "@tanstack/react-query";
import { Ionicons } from "@expo/vector-icons";
import { useColorScheme } from "nativewind";
import type {
  Agent,
  Issue,
  MemberWithUser,
  Team,
} from "@patchbay/core/types";
import { Text } from "@/components/ui/text";
import { ActorAvatar } from "@/components/ui/actor-avatar";
import { StatusIcon } from "@/components/ui/status-icon";
import { issueColumnCategory } from "@/lib/issue-status";
import { useIssueStatuses } from "@/lib/use-issue-statuses";
import { memberListOptions } from "@/data/queries/members";
import { agentListOptions } from "@/data/queries/agents";
import { teamListOptions } from "@/data/queries/teams";
import { api } from "@/data/api";
import { useWorkspaceStore } from "@/data/workspace-store";
import {
  useMentionDraftStore,
  type MentionChipDraft,
  type MentionTargetType,
} from "@/data/stores/mention-draft-store";
import { useScrollToTopOnChange } from "@/lib/use-scroll-to-top-on-change";
import { THEME } from "@/lib/theme";
import { isAgentRuntimeBound } from "@/lib/is-agent-runtime-bound";
import { cn } from "@/lib/utils";

const AVATAR_SIZE = 36;

type Row =
  | { kind: "section"; label: string }
  | { kind: "all" }
  | { kind: "member"; member: MemberWithUser }
  | { kind: "agent"; agent: Agent }
  | { kind: "team"; team: Team }
  | { kind: "issue"; issue: Issue };

interface Props {
  query: string;
  /** "comment" (default) renders @all + People + Agents + Teams + Issues.
   *  "chat" hides the people-style sections (member / agent / team /
   *  @all) because chat is user ↔ single agent — mentioning a person
   *  there generates unintended notifications. Only Issues remain useful
   *  in chat as "reference this ticket for context". */
  mode?: "comment" | "chat";
}

export function MentionPickerBody({ query, mode = "comment" }: Props) {
  const wsId = useWorkspaceStore((s) => s.currentWorkspaceId);
  // Rows are icon-only here too — colour is what names a custom status.
  const catalog = useIssueStatuses();
  const { data: members = [] } = useQuery(memberListOptions(wsId));
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
  const checkColor =
    colorScheme === "dark" ? THEME.dark.primary : THEME.light.primary;

  const selected = useMentionDraftStore((s) => s.mentions);
  const toggle = useMentionDraftStore((s) => s.toggle);

  // Server-side issue search (mirrors web's mention-suggestion.tsx). Empty
  // query → no fetch + no issues section. Debounced 200ms; in-flight
  // cancelled on every keystroke via AbortController.
  const [issueResults, setIssueResults] = useState<Issue[]>([]);
  useEffect(() => {
    const trimmed = query.trim();
    if (!trimmed) {
      setIssueResults([]);
      return;
    }
    const ac = new AbortController();
    const timer = setTimeout(() => {
      void api
        .searchIssues(
          { q: trimmed, limit: 8, include_closed: false },
          { signal: ac.signal },
        )
        .then((res) => setIssueResults(res.issues))
        .catch(() => setIssueResults([]));
    }, 200);
    return () => {
      ac.abort();
      clearTimeout(timer);
    };
  }, [query]);

  const isSelectedKey = (type: MentionTargetType, id: string) =>
    selected.some((m) => m.type === type && m.id === id);

  const isSelected = (row: Row): boolean => {
    if (row.kind === "section") return false;
    if (row.kind === "all") return isSelectedKey("all", "all");
    if (row.kind === "member")
      return isSelectedKey("member", row.member.user_id);
    if (row.kind === "agent") return isSelectedKey("agent", row.agent.id);
    if (row.kind === "team") return isSelectedKey("team", row.team.id);
    return isSelectedKey("issue", row.issue.id);
  };

  const rows = useMemo<Row[]>(() => {
    const q = query.trim().toLowerCase();
    const matchName = (name: string) => !q || name.toLowerCase().includes(q);

    const out: Row[] = [];

    // People-style sections only render in comment mode. Chat is single-
    // agent; @张三 / @team / @all there are noise + notify the wrong
    // people. The Issues section IS useful in chat ("reference ticket
    // for context"), so it stays for both modes.
    if (mode === "comment") {
      if (!q || "all".includes(q)) {
        out.push({ kind: "all" });
      }
      const memberRows = [...members]
        .filter((m) => matchName(m.name))
        .sort((a, b) => a.name.localeCompare(b.name))
        .map((m): Row => ({ kind: "member", member: m }));
      if (memberRows.length > 0) {
        out.push({ kind: "section", label: "People" }, ...memberRows);
      }
      const agentRows = [...agents]
        .filter((a) => matchName(a.name))
        .sort((a, b) => a.name.localeCompare(b.name))
        .map((a): Row => ({ kind: "agent", agent: a }));
      if (agentRows.length > 0) {
        out.push({ kind: "section", label: "Agents" }, ...agentRows);
      }
      const teamRows = [...teams]
        .filter((s) => !s.archived_at && matchName(s.name))
        .sort((a, b) => a.name.localeCompare(b.name))
        .map((s): Row => ({ kind: "team", team: s }));
      if (teamRows.length > 0) {
        out.push({ kind: "section", label: "Teams" }, ...teamRows);
      }
    }

    if (issueResults.length > 0) {
      out.push({ kind: "section", label: "Issues" });
      for (const i of issueResults) {
        out.push({ kind: "issue", issue: i });
      }
    }
    return out;
  }, [mode, members, agents, teams, issueResults, query]);

  const pick = (row: Row) => {
    let chip: MentionChipDraft | null = null;
    if (row.kind === "all") chip = { type: "all", id: "all", name: "all" };
    else if (row.kind === "member")
      chip = {
        type: "member",
        id: row.member.user_id,
        name: row.member.name,
      };
    else if (row.kind === "agent")
      chip = { type: "agent", id: row.agent.id, name: row.agent.name };
    else if (row.kind === "team")
      chip = { type: "team", id: row.team.id, name: row.team.name };
    else if (row.kind === "issue")
      chip = {
        type: "issue",
        id: row.issue.id,
        name: row.issue.identifier,
      };
    if (chip) toggle(chip);
  };

  // FlatList returned as the route's direct child so RNSScreenContentWrapper
  // can find it as a direct subview and apply the iOS formSheet header
  // offset. See react-native-screens#3634.
  return (
    <FlatList
      ref={listRef}
      data={rows}
      className="flex-1"
      keyboardShouldPersistTaps="handled"
      automaticallyAdjustKeyboardInsets
      contentInsetAdjustmentBehavior="automatic"
      keyExtractor={(row, idx) => {
        if (row.kind === "section") return `section:${row.label}:${idx}`;
        if (row.kind === "all") return "all";
        if (row.kind === "member") return `m:${row.member.user_id}`;
        if (row.kind === "agent") return `a:${row.agent.id}`;
        if (row.kind === "team") return `s:${row.team.id}`;
        return `i:${row.issue.id}`;
      }}
      renderItem={({ item }) => {
        if (item.kind === "section") {
          return (
            <View className="px-4 pt-4 pb-1">
              <Text className="text-[11px] font-medium uppercase tracking-wide text-muted-foreground">
                {item.label}
              </Text>
            </View>
          );
        }
        const needsRuntime =
          (item.kind === "agent" && !isAgentRuntimeBound(item.agent)) ||
          (item.kind === "team" &&
            !runnableAgentIds.has(item.team.leader_id));
        return (
          <Pressable
            disabled={needsRuntime}
            onPress={() => pick(item)}
            className={cn(
              "flex-row items-center gap-3 px-4 py-3 active:bg-secondary",
              needsRuntime && "opacity-50",
            )}
          >
            {item.kind === "all" ? (
              <View
                className="rounded-full bg-primary/10 items-center justify-center"
                style={{ width: AVATAR_SIZE, height: AVATAR_SIZE }}
              >
                <Ionicons name="people" size={20} color={checkColor} />
              </View>
            ) : item.kind === "member" ? (
              <ActorAvatar
                type="member"
                id={item.member.user_id}
                size={AVATAR_SIZE}
              />
            ) : item.kind === "agent" ? (
              <ActorAvatar type="agent" id={item.agent.id} size={AVATAR_SIZE} />
            ) : item.kind === "team" ? (
              <ActorAvatar type="team" id={item.team.id} size={AVATAR_SIZE} />
            ) : (
              <View
                className="items-center justify-center"
                style={{ width: AVATAR_SIZE, height: AVATAR_SIZE }}
              >
                <StatusIcon
                  status={item.issue.status}
                  category={issueColumnCategory(item.issue)}
                  color={catalog.colorOf(item.issue.status)}
                  size={22}
                />
              </View>
            )}
            {item.kind === "issue" ? (
              <View className="flex-1 flex-row items-center gap-2">
                <Text className="text-sm font-medium text-muted-foreground">
                  {item.issue.identifier}
                </Text>
                <Text
                  className="flex-1 text-base text-foreground"
                  numberOfLines={1}
                >
                  {item.issue.title}
                </Text>
              </View>
            ) : (
              <Text className="flex-1 text-base text-foreground">
                {item.kind === "all"
                  ? "Everyone (@all)"
                  : item.kind === "member"
                    ? item.member.name
                    : item.kind === "agent"
                      ? item.agent.name
                      : item.team.name}
              </Text>
            )}
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
