/**
 * Final step of an existing issue's review handoff. The status/reviewer
 * pickers route here without writing first, so the server receives reviewer,
 * status, and delivery evidence as one atomic update.
 */
import { Alert } from "react-native";
import { router, useLocalSearchParams } from "expo-router";
import { useQuery } from "@tanstack/react-query";
import type { IssueActorType } from "@orvilo/core/types";
import { ReviewSubmissionForm } from "@/components/issue/review-submission-form";
import { useAuthStore } from "@/data/auth-store";
import { useUpdateIssue } from "@/data/mutations/issues";
import { issueDetailOptions } from "@/data/queries/issues";
import { useWorkspaceStore } from "@/data/workspace-store";
import type { IssueRoleRef } from "@/lib/issue-review-workflow";
import { getIssueRoleCopy } from "@/lib/issue-role-copy";
import { issueActorForRole } from "@/lib/issue-scope";

function reviewerFromParams(
  type: string | undefined,
  id: string | undefined,
): IssueRoleRef | null {
  if (id && (type === "member" || type === "agent" || type === "team")) {
    return { type: type as IssueActorType, id };
  }
  return null;
}

export default function IssueReviewSubmissionRoute() {
  const { id, handoffStatus, reviewerType, reviewerId } = useLocalSearchParams<{
    id: string;
    handoffStatus?: string;
    reviewerType?: string;
    reviewerId?: string;
  }>();
  const wsId = useWorkspaceStore((state) => state.currentWorkspaceId);
  const language = useAuthStore((state) => state.user?.language);
  const copy = getIssueRoleCopy(language);
  const { data: issue } = useQuery(issueDetailOptions(wsId, id));
  const updateIssue = useUpdateIssue(id);
  const reviewer =
    reviewerFromParams(reviewerType, reviewerId) ??
    (issue ? issueActorForRole(issue, "reviewer") : null);

  if (!handoffStatus || !reviewer) return null;

  return (
    <ReviewSubmissionForm
      status={handoffStatus}
      reviewer={reviewer}
      submitting={updateIssue.isPending}
      onSubmit={(patch) => {
        updateIssue.mutate(patch, {
          onSuccess: () => router.back(),
          onError: (error) =>
            Alert.alert(
              copy.updateFailed,
              error instanceof Error ? error.message : copy.updateFailed,
            ),
        });
      }}
    />
  );
}
