/** Collect a complete review packet before storing a review-status draft. */
import { router, useLocalSearchParams } from "expo-router";
import type { IssueActorType } from "@orvilo/core/types";
import { ReviewSubmissionForm } from "@/components/issue/review-submission-form";
import { useNewIssueDraftStore } from "@/data/stores/new-issue-draft-store";
import type { IssueRoleRef } from "@/lib/issue-review-workflow";

function reviewerFromParams(
  type: string | undefined,
  id: string | undefined,
): IssueRoleRef | null {
  if (id && (type === "member" || type === "agent" || type === "team")) {
    return { type: type as IssueActorType, id };
  }
  return null;
}

export default function NewIssueReviewSubmissionRoute() {
  const { handoffStatus, reviewerType, reviewerId } = useLocalSearchParams<{
    handoffStatus?: string;
    reviewerType?: string;
    reviewerId?: string;
  }>();
  const currentReviewer = useNewIssueDraftStore((state) => state.reviewer);
  const setReviewHandoff = useNewIssueDraftStore(
    (state) => state.setReviewHandoff,
  );
  const reviewer =
    reviewerFromParams(reviewerType, reviewerId) ?? currentReviewer;

  if (!handoffStatus || !reviewer) return null;

  return (
    <ReviewSubmissionForm
      status={handoffStatus}
      reviewer={reviewer}
      submitting={false}
      onSubmit={(patch) => {
        setReviewHandoff(
          patch.status,
          {
            type: patch.reviewer_type,
            id: patch.reviewer_id,
          },
          patch.review_submission,
        );
        router.back();
      }}
    />
  );
}
