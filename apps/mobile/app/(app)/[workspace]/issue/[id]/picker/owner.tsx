import { router, Stack, useLocalSearchParams } from "expo-router";
import { useQuery } from "@tanstack/react-query";
import { RolePickerBody } from "@/components/issue/pickers/role-picker-body";
import { useAuthStore } from "@/data/auth-store";
import { useUpdateIssue } from "@/data/mutations/issues";
import { issueDetailOptions } from "@/data/queries/issues";
import { useWorkspaceStore } from "@/data/workspace-store";
import { getIssueRoleCopy } from "@/lib/issue-role-copy";
import { ownerPatch } from "@/lib/issue-role-patch";
import { useNativeSearchBar } from "@/lib/use-native-search-bar";

export default function IssueOwnerPickerRoute() {
  const { id } = useLocalSearchParams<{ id: string }>();
  const wsId = useWorkspaceStore((state) => state.currentWorkspaceId);
  const language = useAuthStore((state) => state.user?.language);
  const copy = getIssueRoleCopy(language);
  const { data: issue } = useQuery(issueDetailOptions(wsId, id));
  const updateIssue = useUpdateIssue(id);
  const query = useNativeSearchBar(copy.searchMembers, { autoFocus: true });
  const value =
    issue?.owner_type === "member" && issue.owner_id
      ? { type: "member" as const, id: issue.owner_id }
      : null;

  return (
    <>
      <Stack.Screen options={{ title: copy.owner }} />
      <RolePickerBody
        kind="owner"
        value={value}
        query={query}
        onChange={(next) => {
          updateIssue.mutate(ownerPatch(next));
          router.back();
        }}
      />
    </>
  );
}
