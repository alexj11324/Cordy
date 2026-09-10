import { router, Stack, useLocalSearchParams } from "expo-router";
import { RolePickerBody } from "@/components/issue/pickers/role-picker-body";
import { useAuthStore } from "@/data/auth-store";
import { useNewIssueDraftStore } from "@/data/stores/new-issue-draft-store";
import { getIssueRoleCopy } from "@/lib/issue-role-copy";
import { isReviewHandoff } from "@/lib/issue-review-workflow";
import { useNativeSearchBar } from "@/lib/use-native-search-bar";
import { useIssueStatuses } from "@/lib/use-issue-statuses";

export default function NewIssueReviewerPickerRoute() {
  const { workspace, handoffStatus } = useLocalSearchParams<{
    workspace: string;
    handoffStatus?: string;
  }>();
  const reviewer = useNewIssueDraftStore((state) => state.reviewer);
  const setReviewer = useNewIssueDraftStore((state) => state.setReviewer);
  const executor = useNewIssueDraftStore((state) => state.executor);
  const status = useNewIssueDraftStore((state) => state.status);
  const language = useAuthStore((state) => state.user?.language);
  const copy = getIssueRoleCopy(language);
  const catalog = useIssueStatuses();
  const query = useNativeSearchBar(copy.searchReviewers, { autoFocus: true });
  const isHandoff =
    !!handoffStatus && isReviewHandoff(null, catalog.categoryOf(handoffStatus));

  return (
    <>
      <Stack.Screen
        options={{ title: isHandoff ? copy.reviewHandoff : copy.reviewer }}
      />
      <RolePickerBody
        kind="reviewer"
        value={reviewer}
        query={query}
        allowUnassigned={!isHandoff}
        excludedActor={
          isHandoff || catalog.categoryOf(status) === "in_review"
            ? executor
            : null
        }
        onChange={(next) => {
          if (isHandoff && handoffStatus && next) {
            router.replace({
              pathname: "/[workspace]/new-issue-picker/review-submission",
              params: {
                workspace,
                handoffStatus,
                reviewerType: next.type,
                reviewerId: next.id,
              },
            });
            return;
          }
          setReviewer(next);
          router.back();
        }}
      />
    </>
  );
}
