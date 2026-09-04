/**
 * Status picker route for an existing issue — presented as a formSheet
 * (UISheetPresentationController) by the parent Stack.
 *
 * Self-contained: reads the issue from the TanStack Query detail cache,
 * calls `useUpdateIssue` directly on selection, then `router.back()`s. No
 * onChange callback to a parent.
 *
 * If the cache is cold (rare — the user reaches this screen by tapping
 * a chip on the issue-detail page that already populated it), the picker
 * still renders against the current value of `todo` and the optimistic
 * mutation patches the cache when the user picks.
 */
import { Alert } from "react-native";
import { useLocalSearchParams, router } from "expo-router";
import { useQuery } from "@tanstack/react-query";
import { StatusPickerBody } from "@/components/issue/pickers/status-picker-body";
import { issueDetailOptions } from "@/data/queries/issues";
import { useUpdateIssue } from "@/data/mutations/issues";
import { useAuthStore } from "@/data/auth-store";
import { useWorkspaceStore } from "@/data/workspace-store";
import { getIssueRoleCopy } from "@/lib/issue-role-copy";
import { issueActorForRole } from "@/lib/issue-scope";
import { issueStatusCategory } from "@/lib/issue-status";
import { reviewWorkflowViolation } from "@/lib/issue-review-workflow";
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
  const { data: issue } = useQuery(issueDetailOptions(wsId, id));
  const updateIssue = useUpdateIssue(id);

  return (
    <StatusPickerBody
      value={issue?.status ?? "todo"}
      onChange={(next) => {
        if (!issue) return;
        const previousCategory =
          issueStatusCategory(issue) ?? catalog.categoryOf(issue.status);
        const violation = reviewWorkflowViolation({
          previousCategory,
          nextCategory: catalog.categoryOf(next),
          executor: issueActorForRole(issue, "executor"),
          reviewer: issueActorForRole(issue, "reviewer"),
        });
        if (violation === "executor_required") {
          Alert.alert(copy.executor, copy.executorRequired);
          return;
        }
        if (
          violation === "reviewer_required" ||
          violation === "reviewer_must_differ"
        ) {
          router.replace({
            pathname: "/[workspace]/issue/[id]/picker/reviewer",
            params: { workspace, id, handoffStatus: next },
          });
          return;
        }
        updateIssue.mutate(
          { status: next },
          {
            onSuccess: () => router.back(),
            onError: (error) =>
              Alert.alert(
                copy.updateFailed,
                error instanceof Error ? error.message : copy.updateFailed,
              ),
          },
        );
      }}
    />
  );
}
