import { useState } from "react";
import {
  ActivityIndicator,
  Pressable,
  RefreshControl,
  ScrollView,
  View,
} from "react-native";
import { router, Stack } from "expo-router";
import { useQuery } from "@tanstack/react-query";
import type {
  DependencyGraphEdge,
  DependencyGraphNode,
  DependencyGraphResponse,
} from "@patchbay/core/types";
import { Text } from "@/components/ui/text";
import { Button } from "@/components/ui/button";
import { dependencyGraphsOptions } from "@/data/queries/dependency-graphs";
import { useWorkspaceStore } from "@/data/workspace-store";

type GraphFilter = "all" | "ready" | "running" | "blocked";

function nodeState(node: DependencyGraphNode): string {
  return node.readiness?.state || node.status || "todo";
}

function nodeIdentifier(node: DependencyGraphNode): string {
  return node.issue?.identifier || node.issue_id || node.temp_id || node.id;
}

function nodeTitle(node: DependencyGraphNode): string {
  return node.title.trim() || node.issue?.title || nodeIdentifier(node);
}

function edgeEndpoint(edge: DependencyGraphEdge, side: "from" | "to"): string {
  return side === "from"
    ? edge.from || edge.from_issue_id
    : edge.to || edge.to_issue_id;
}

function edgeLabel(graph: DependencyGraphResponse, endpoint: string): string {
  const node = graph.nodes.find(
    (candidate) =>
      candidate.temp_id === endpoint ||
      candidate.issue_id === endpoint ||
      candidate.issue?.id === endpoint ||
      candidate.issue?.identifier === endpoint,
  );
  return node ? nodeIdentifier(node) : endpoint || "?";
}

function stateTone(state: string): string {
  switch (state) {
    case "ready":
      return "text-emerald-700 dark:text-emerald-300";
    case "running":
      return "text-blue-700 dark:text-blue-300";
    case "blocked":
      return "text-amber-700 dark:text-amber-300";
    case "done":
      return "text-muted-foreground";
    default:
      return "text-foreground";
  }
}

function stateLabel(state: string): string {
  switch (state) {
    case "ready":
      return "Ready";
    case "running":
      return "Running";
    case "blocked":
      return "Blocked";
    case "done":
      return "Done";
    case "cancelled":
      return "Cancelled";
    default:
      return state || "Todo";
  }
}

function filterMatches(node: DependencyGraphNode, filter: GraphFilter): boolean {
  return filter === "all" || nodeState(node) === filter;
}

function summary(graphs: DependencyGraphResponse[]) {
  return graphs.reduce(
    (result, graph) => {
      for (const node of graph.nodes) {
        const state = nodeState(node);
        result.total += 1;
        if (state === "ready") result.ready += 1;
        if (state === "running") result.running += 1;
        if (state === "blocked") result.blocked += 1;
      }
      if (graph.nodes.length === 0) {
        result.total += graph.readiness.total;
        result.ready += graph.readiness.ready;
        result.running += graph.readiness.running;
        result.blocked += graph.readiness.blocked;
      }
      return result;
    },
    { total: 0, ready: 0, running: 0, blocked: 0 },
  );
}

export default function TaskGraphScreen() {
  const wsId = useWorkspaceStore((state) => state.currentWorkspaceId);
  const wsSlug = useWorkspaceStore((state) => state.currentWorkspaceSlug);
  const {
    data: graphs = [],
    isLoading,
    error,
    refetch,
    isRefetching,
  } = useQuery(dependencyGraphsOptions(wsId));
  const [filter, setFilter] = useState<GraphFilter>("all");

  const totals = summary(graphs);
  const visibleGraphs = graphs.filter(
    (graph) =>
      filter === "all" || graph.nodes.some((node) => filterMatches(node, filter)),
  );

  return (
    <View className="flex-1 bg-background">
      <Stack.Screen options={{ title: "Dependency Graph", headerBackTitle: "Back" }} />
      {isLoading ? (
        <View className="flex-1 items-center justify-center">
          <ActivityIndicator />
        </View>
      ) : error ? (
        <View className="gap-3 px-4 pt-5">
          <Text className="text-sm text-destructive">
            Failed to load dependency graphs: {error instanceof Error ? error.message : "unknown error"}
          </Text>
          <Button variant="outline" onPress={() => void refetch()}>
            <Text>Retry</Text>
          </Button>
        </View>
      ) : graphs.length === 0 ? (
        <View className="flex-1 items-center justify-center gap-2 px-6">
          <Text className="text-base font-medium">No dependency graphs yet</Text>
          <Text className="text-center text-sm text-muted-foreground">
            Apply a dependency graph to a parent task to see its execution plan here.
          </Text>
        </View>
      ) : (
        <ScrollView
          contentContainerClassName="gap-4 px-4 pb-8 pt-4"
          refreshControl={
            <RefreshControl
              refreshing={isRefetching}
              onRefresh={() => void refetch()}
            />
          }
        >
          <View className="gap-1 rounded-xl border border-border bg-card p-4">
            <Text className="text-base font-medium">{graphs.length} active plans</Text>
            <Text className="text-sm text-muted-foreground">
              {totals.total} tasks · {totals.ready} ready · {totals.running} running · {totals.blocked} blocked
            </Text>
            <View className="mt-3 flex-row flex-wrap gap-2">
              {(["all", "ready", "running", "blocked"] as const).map((value) => (
                <Button
                  key={value}
                  size="sm"
                  variant={filter === value ? "secondary" : "outline"}
                  onPress={() => setFilter(value)}
                >
                  <Text>{value[0].toUpperCase() + value.slice(1)}</Text>
                </Button>
              ))}
            </View>
          </View>

          {visibleGraphs.map((graph) => {
            const nodes = graph.nodes.filter((node) => filterMatches(node, filter));
            const waves = Array.from(new Set(nodes.map((node) => node.wave))).sort(
              (left, right) => left - right,
            );
            return (
              <View key={graph.plan.id} className="gap-3 rounded-xl border border-border bg-card p-4">
                <View className="gap-1">
                  <View className="flex-row items-center justify-between gap-2">
                    <Text className="min-w-0 flex-1 text-base font-medium">
                      Plan · {graph.plan.id.slice(0, 8)}
                    </Text>
                    <Text className={stateTone(graph.plan.status)}>
                      {stateLabel(graph.plan.status)}
                    </Text>
                  </View>
                  <Text className="text-sm text-muted-foreground">
                    {graph.plan.goal || "Dependency graph execution plan"}
                  </Text>
                </View>

                {graph.plan.attention_required ? (
                  <View className="rounded-lg bg-amber-500/10 p-3">
                    <Text className="text-sm text-amber-800 dark:text-amber-200">
                      Planner attention required: {graph.plan.attention_reason || "review the execution gate"}
                    </Text>
                  </View>
                ) : null}

                {waves.length > 0 ? (
                  waves.map((wave) => (
                    <View key={wave} className="gap-2">
                      <Text className="text-sm font-medium text-muted-foreground">Wave {wave}</Text>
                      {nodes
                        .filter((node) => node.wave === wave)
                        .map((node) => {
                          const readiness = node.readiness;
                          const identifier = nodeIdentifier(node);
                          return (
                            <Pressable
                              key={node.id || node.temp_id}
                              className="gap-2 rounded-lg border border-border bg-background p-3 active:bg-accent"
                              accessibilityRole="button"
                              accessibilityLabel={`Open ${identifier}`}
                              onPress={() => {
                                if (wsSlug) router.push(`/${wsSlug}/issue/${node.issue_id || node.temp_id}`);
                              }}
                            >
                              <View className="flex-row items-start justify-between gap-2">
                                <Text className="flex-1 text-sm font-medium text-primary">
                                  {identifier}
                                </Text>
                                <Text className={stateTone(nodeState(node))}>
                                  {stateLabel(nodeState(node))}
                                </Text>
                              </View>
                              <Text className="text-sm">{nodeTitle(node)}</Text>
                              <Text className="text-xs text-muted-foreground">
                                {readiness.gate_open ? "Gate open" : "Gate blocked"} · {readiness.satisfied_prerequisites}/{readiness.total_prerequisites} prerequisites satisfied
                              </Text>
                            </Pressable>
                          );
                        })}
                    </View>
                  ))
                ) : (
                  <Text className="py-4 text-center text-sm text-muted-foreground">
                    No tasks match this filter.
                  </Text>
                )}

                {graph.edges.length > 0 ? (
                  <View className="gap-2 border-t border-border pt-3">
                    <Text className="text-sm font-medium">Dependencies</Text>
                    {graph.edges.map((edge) => (
                      <View key={edge.id} className="rounded-lg bg-muted/30 p-3">
                        <Text className="text-sm">
                          {edgeLabel(graph, edgeEndpoint(edge, "from"))} → {edgeLabel(graph, edgeEndpoint(edge, "to"))}
                        </Text>
                        <Text className={cnEdgeStatus(edge.satisfied)}>
                          {edge.satisfied ? "Satisfied" : "Blocked"}
                        </Text>
                      </View>
                    ))}
                  </View>
                ) : null}
              </View>
            );
          })}
          {visibleGraphs.length === 0 ? (
            <Text className="py-8 text-center text-sm text-muted-foreground">
              No tasks match this filter.
            </Text>
          ) : null}
        </ScrollView>
      )}
    </View>
  );
}

function cnEdgeStatus(satisfied: boolean): string {
  return satisfied
    ? "text-xs text-emerald-700 dark:text-emerald-300"
    : "text-xs text-amber-700 dark:text-amber-300";
}
