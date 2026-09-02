import { router, Stack, useLocalSearchParams } from "expo-router";
import { useQuery } from "@tanstack/react-query";
import { RolePickerBody } from "@/components/issue/pickers/role-picker-body";
import { useAuthStore } from "@/data/auth-store";
import { useUpdateIssue } from "@/data/mutations/issues";
import { issueDetailOptions } from "@/data/queries/issues";
import { useWorkspaceStore } from "@/data/workspace-store";
import { getIssueRoleCopy } from "@/lib/issue-role-copy";
import { reviewerPatch } from "@/lib/issue-role-patch";
import { useNativeSearchBar } from "@/lib/use-native-search-bar";

export default function IssueReviewerPickerRoute() {
  const { id } = useLocalSearchParams<{ id: string }>();
  const wsId = useWorkspaceStore((state) => state.currentWorkspaceId);
  const language = useAuthStore((state) => state.user?.language);
  const copy = getIssueRoleCopy(language);
  const { data: issue } = useQuery(issueDetailOptions(wsId, id));
  const updateIssue = useUpdateIssue(id);
  const query = useNativeSearchBar(copy.searchReviewers, { autoFocus: true });
  const value =
    issue?.reviewer_type && issue.reviewer_id
      ? { type: issue.reviewer_type, id: issue.reviewer_id }
      : null;

  return (
    <>
      <Stack.Screen options={{ title: copy.reviewer }} />
      <RolePickerBody
        kind="reviewer"
        value={value}
        query={query}
        onChange={(next) => {
          updateIssue.mutate(reviewerPatch(next));
          router.back();
        }}
      />
    </>
  );
}
