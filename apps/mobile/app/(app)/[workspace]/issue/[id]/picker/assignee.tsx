/**
 * Assignee picker route for an existing issue. Uses the native iOS Stack
 * header + UISearchController (registered in ../_layout.tsx with
 * `headerShown: true` + title); the search bar wiring is encapsulated in
 * `useNativeSearchBar`.
 */
import { useLocalSearchParams, router } from "expo-router";
import { useQuery } from "@tanstack/react-query";
import { AssigneePickerBody } from "@/components/issue/pickers/assignee-picker-body";
import { issueDetailOptions } from "@/data/queries/issues";
import { useUpdateIssue } from "@/data/mutations/issues";
import { useWorkspaceStore } from "@/data/workspace-store";
import { useNativeSearchBar } from "@/lib/use-native-search-bar";

export default function IssueAssigneePickerRoute() {
  const { id } = useLocalSearchParams<{ id: string }>();
  const wsId = useWorkspaceStore((s) => s.currentWorkspaceId);
  const { data: issue } = useQuery(issueDetailOptions(wsId, id));
  const updateIssue = useUpdateIssue(id);
  const query = useNativeSearchBar("Search people", { autoFocus: true });

  const value =
    issue?.executor_type && issue?.executor_id
      ? { type: issue.executor_type, id: issue.executor_id }
      : issue?.owner_type && issue?.owner_id
        ? { type: issue.owner_type, id: issue.owner_id }
        : null;

  return (
    <AssigneePickerBody
      value={value}
      query={query}
      onChange={(next) => {
        if (next === null) {
          updateIssue.mutate({
            owner_type: null,
            owner_id: null,
            executor_type: null,
            executor_id: null,
          });
        } else if (next.type === "member") {
          updateIssue.mutate({
            owner_type: "member",
            owner_id: next.id,
            executor_type: null,
            executor_id: null,
          });
        } else {
          updateIssue.mutate({
            executor_type: next.type,
            executor_id: next.id,
          });
        }
        router.back();
      }}
    />
  );
}
