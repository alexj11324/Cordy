import { router, useLocalSearchParams } from "expo-router";
import { useQuery } from "@tanstack/react-query";
import { RolePickerBody } from "@/components/issue/pickers/role-picker-body";
import { issueDetailOptions } from "@/data/queries/issues";
import { useUpdateIssue } from "@/data/mutations/issues";
import { useWorkspaceStore } from "@/data/workspace-store";
import { useNativeSearchBar } from "@/lib/use-native-search-bar";

export default function IssueOwnerPickerRoute() {
  const { id } = useLocalSearchParams<{ id: string }>();
  const wsId = useWorkspaceStore((s) => s.currentWorkspaceId);
  const { data: issue } = useQuery(issueDetailOptions(wsId, id));
  const updateIssue = useUpdateIssue(id);
  const query = useNativeSearchBar("Search members", { autoFocus: true });
  const value = issue?.owner_type === "member" && issue.owner_id
    ? { type: "member" as const, id: issue.owner_id }
    : null;

  return (
    <RolePickerBody
      kind="owner"
      value={value}
      query={query}
      onChange={(next) => {
        updateIssue.mutate({
          owner_type: next?.type === "member" ? "member" : null,
          owner_id: next?.type === "member" ? next.id : null,
        });
        router.back();
      }}
    />
  );
}
