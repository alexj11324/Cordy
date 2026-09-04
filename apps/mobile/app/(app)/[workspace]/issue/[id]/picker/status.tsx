/**
 * Status picker route for an existing issue — presented as a formSheet
 * (UISheetPresentationController) by the parent Stack.
 *
 * Self-contained: reads the issue from the TanStack Query detail cache,
 * calls `useUpdateIssue` directly on selection, then `router.back()`s. No
 * onChange callback to a parent.
 *
 * If the cache is cold, the picker waits for the issue instead of accepting a
 * selection against guessed role/status state. That keeps handoff validation
 * tied to the same snapshot the user sees.
 */
import { useRef } from "react";
import { ActivityIndicator, Alert, View } from "react-native";
import { useLocalSearchParams, router } from "expo-router";
import { useQuery } from "@tanstack/react-query";
import { StatusPickerBody } from "@/components/issue/pickers/status-picker-body";
import { Button } from "@/components/ui/button";
import { Text } from "@/components/ui/text";
import { issueDetailOptions } from "@/data/queries/issues";
import { useUpdateIssue } from "@/data/mutations/issues";
import { useAuthStore } from "@/data/auth-store";
import { useWorkspaceStore } from "@/data/workspace-store";
import { getIssueRoleCopy } from "@/lib/issue-role-copy";
import { issueActorForRole } from "@/lib/issue-scope";
import { issueStatusCategory } from "@/lib/issue-status";
import { planIssueStatusSelection } from "@/lib/issue-review-workflow";
import { useIssueStatuses } from "@/lib/use-issue-statuses";

export default function IssueStatusPickerRoute() {
  const { id, workspace } = useLocalSearchParams<{
    id: string;
    workspace: string;
  }>();
  const wsId = useWorkspaceStore((s) => s.currentWorkspaceId);
  const language = useAuthStore((s) => s.user?.language);
  const copy = getIssueRoleCopy(language);
  const catalog = useIssueStatuses();
  const detail = useQuery(issueDetailOptions(wsId, id));
  const issue = detail.data;
  const updateIssue = useUpdateIssue(id);
  const writingRef = useRef(false);

  if (detail.isLoading) {
    return (
      <View className="flex-1 items-center justify-center bg-background">
        <ActivityIndicator />
      </View>
    );
  }
  if (detail.error || !issue) {
    return (
      <View className="flex-1 items-center justify-center gap-3 bg-background px-6">
        <Text className="text-sm text-destructive">{copy.loadIssueFailed}</Text>
        <Button variant="outline" onPress={() => detail.refetch()}>
          <Text>{copy.retry}</Text>
        </Button>
      </View>
    );
  }

  return (
    <StatusPickerBody
      value={issue.status}
      disabled={updateIssue.isPending}
      onChange={(next) => {
        if (writingRef.current) return;
        const previousCategory =
          issueStatusCategory(issue) ?? catalog.categoryOf(issue.status);
        const plan = planIssueStatusSelection({
          previousCategory,
          nextStatus: next,
          nextCategory: catalog.categoryOf(next),
          executor: issueActorForRole(issue, "executor"),
          reviewer: issueActorForRole(issue, "reviewer"),
        });
        if (plan.kind === "blocked") {
          Alert.alert(copy.executor, copy.executorRequired);
          return;
        }
        if (plan.kind === "choose_reviewer") {
          router.replace({
            pathname: "/[workspace]/issue/[id]/picker/reviewer",
            params: { workspace, id, handoffStatus: plan.status },
          });
          return;
        }
        writingRef.current = true;
        updateIssue.mutate(
          { status: plan.status },
          {
            onSuccess: () => router.back(),
            onError: (error) =>
              Alert.alert(
                copy.updateFailed,
                error instanceof Error ? error.message : copy.updateFailed,
              ),
            onSettled: () => {
              writingRef.current = false;
            },
          },
        );
      }}
    />
  );
}
