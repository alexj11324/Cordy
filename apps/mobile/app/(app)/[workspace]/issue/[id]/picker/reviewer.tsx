import { router, useLocalSearchParams } from "expo-router";
import { useQuery } from "@tanstack/react-query";
import { RolePickerBody } from "@/components/issue/pickers/role-picker-body";
import { issueDetailOptions } from "@/data/queries/issues";
import { useUpdateIssue } from "@/data/mutations/issues";
import { useWorkspaceStore } from "@/data/workspace-store";
import { useNativeSearchBar } from "@/lib/use-native-search-bar";

export default function IssueReviewerPickerRoute() {
  const { id } = useLocalSearchParams<{ id: string }>();
  const wsId = useWorkspaceStore((s) => s.currentWorkspaceId);
  const { data: issue } = useQuery(issueDetailOptions(wsId, id));
  const updateIssue = useUpdateIssue(id);
  const query = useNativeSearchBar("Search reviewers", { autoFocus: true });
  const value = issue?.reviewer_type && issue.reviewer_id
    ? { type: issue.reviewer_type, id: issue.reviewer_id }
    : null;

  return (
    <RolePickerBody
      kind="reviewer"
      value={value}
      query={query}
      onChange={(next) => {
        updateIssue.mutate({
          reviewer_type: next?.type ?? null,
          reviewer_id: next?.id ?? null,
        });
        router.back();
      }}
    />
  );
}
