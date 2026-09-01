/**
 * Agent Runs sheet — presented as a formSheet by the parent Stack. Two
 * sections: Active (queued/deferred/dispatched/running, created_at desc) and Past
 * (completed_at desc, status rank as tiebreaker). Empty
 * sections hide entirely.
 *
 * Both entry points (the in-card AgentActivityRow and the Stack-header
 * AgentHeaderBadge) now `router.push("/[workspace]/issue/[id]/runs")` —
 * the legacy `useRunsSheetStore` is gone since the route system is the
 * single source of truth for what's open.
 */
import { useMemo } from "react";
import { ScrollView, View } from "react-native";
import { router, useLocalSearchParams } from "expo-router";
import { useQuery } from "@tanstack/react-query";
import { isAgentTaskActive } from "@patchbay/core/agent-thread";
import { Text } from "@/components/ui/text";
import { RunRow } from "@/components/issue/run-row";
import {
  issueActiveTasksOptions,
  issueTasksOptions,
} from "@/data/queries/issues";
import { useWorkspaceStore } from "@/data/workspace-store";
import { useAgentThreadCopy } from "@/lib/use-agent-thread-copy";

const PAST_STATUS_ORDER: Record<string, number> = {
  failed: 0,
  cancelled: 1,
  completed: 2,
  queued: 99,
  dispatched: 99,
  waiting_local_directory: 99,
  waiting_capacity: 99,
  running: 99,
};

export default function IssueRunsRoute() {
  const { id } = useLocalSearchParams<{ id: string }>();
  const wsId = useWorkspaceStore((s) => s.currentWorkspaceId);
  const wsSlug = useWorkspaceStore((s) => s.currentWorkspaceSlug);
  const copy = useAgentThreadCopy();
  const { data: activeTasks = [] } = useQuery(
    issueActiveTasksOptions(wsId, id),
  );
  const { data: allTasks = [] } = useQuery(issueTasksOptions(wsId, id));

  const active = useMemo(
    () =>
      activeTasks
        .filter(isAgentTaskActive)
        .sort((a, b) => (b.created_at ?? "").localeCompare(a.created_at ?? "")),
    [activeTasks],
  );

  const past = useMemo(() => {
    const filtered = allTasks.filter((t) => !isAgentTaskActive(t));
    return filtered.sort((a, b) => {
      const timeDiff = (b.completed_at ?? "").localeCompare(
        a.completed_at ?? "",
      );
      if (timeDiff !== 0) return timeDiff;
      return (
        (PAST_STATUS_ORDER[a.status] ?? 99) -
        (PAST_STATUS_ORDER[b.status] ?? 99)
      );
    });
  }, [allTasks]);

  return (
    <View className="flex-1">
      <View className="px-4 pt-4 pb-3">
        <Text className="text-base font-semibold text-foreground">
          {copy.runs_title}
        </Text>
      </View>
      <ScrollView showsVerticalScrollIndicator={false}>
        <View className="px-4 gap-3 pb-4">
          {active.length > 0 ? (
            <Section title={copy.active}>
              {active.map((task) => (
                <RunRow
                  key={task.id}
                  task={task}
                  issueId={id}
                  onOpen={() => openTaskThread(wsSlug, id, task.id)}
                />
              ))}
            </Section>
          ) : null}
          {past.length > 0 ? (
            <Section title={copy.past}>
              {past.map((task) => (
                <RunRow
                  key={task.id}
                  task={task}
                  issueId={id}
                  onOpen={() => openTaskThread(wsSlug, id, task.id)}
                />
              ))}
            </Section>
          ) : null}
        </View>
      </ScrollView>
    </View>
  );
}

function openTaskThread(
  workspace: string | null,
  issueId: string,
  taskId: string,
) {
  if (!workspace) return;
  router.push({
    pathname: "/[workspace]/issue/[id]/runs/[taskId]",
    params: { workspace, id: issueId, taskId },
  });
}

function Section({
  title,
  children,
}: {
  title: string;
  children: React.ReactNode;
}) {
  return (
    <View className="gap-1">
      <Text className="text-[11px] font-medium text-muted-foreground uppercase tracking-wide">
        {title}
      </Text>
      <View>{children}</View>
    </View>
  );
}
