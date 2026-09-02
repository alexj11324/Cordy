import { router, Stack, useLocalSearchParams } from "expo-router";
import { useQuery } from "@tanstack/react-query";
import { ExecutorPickerBody } from "@/components/issue/pickers/executor-picker-body";
import { useAuthStore } from "@/data/auth-store";
import { useUpdateIssue } from "@/data/mutations/issues";
import { issueDetailOptions } from "@/data/queries/issues";
import { useWorkspaceStore } from "@/data/workspace-store";
import { getIssueRoleCopy } from "@/lib/issue-role-copy";
import { executorPatch } from "@/lib/issue-role-patch";
import { useNativeSearchBar } from "@/lib/use-native-search-bar";

export default function IssueExecutorPickerRoute() {
  const { id } = useLocalSearchParams<{ id: string }>();
  const wsId = useWorkspaceStore((state) => state.currentWorkspaceId);
  const language = useAuthStore((state) => state.user?.language);
  const copy = getIssueRoleCopy(language);
  const { data: issue } = useQuery(issueDetailOptions(wsId, id));
  const updateIssue = useUpdateIssue(id);
  const query = useNativeSearchBar(copy.searchExecutors, { autoFocus: true });
  const value =
    issue?.executor_type && issue.executor_id
      ? { type: issue.executor_type, id: issue.executor_id }
      : null;

  return (
    <>
      <Stack.Screen options={{ title: copy.executor }} />
      <ExecutorPickerBody
        value={value}
        query={query}
        onChange={(next) => {
          updateIssue.mutate(executorPatch(next));
          router.back();
        }}
      />
    </>
  );
}
