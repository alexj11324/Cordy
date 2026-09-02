import { router, Stack } from "expo-router";
import { ExecutorPickerBody } from "@/components/issue/pickers/executor-picker-body";
import { useAuthStore } from "@/data/auth-store";
import { useNewIssueDraftStore } from "@/data/stores/new-issue-draft-store";
import { getIssueRoleCopy } from "@/lib/issue-role-copy";
import { useNativeSearchBar } from "@/lib/use-native-search-bar";

export default function NewIssueExecutorPickerRoute() {
  const executor = useNewIssueDraftStore((state) => state.executor);
  const setExecutor = useNewIssueDraftStore((state) => state.setExecutor);
  const language = useAuthStore((state) => state.user?.language);
  const copy = getIssueRoleCopy(language);
  const query = useNativeSearchBar(copy.searchExecutors, { autoFocus: true });

  return (
    <>
      <Stack.Screen options={{ title: copy.executor }} />
      <ExecutorPickerBody
        value={executor}
        query={query}
        onChange={(next) => {
          setExecutor(next);
          router.back();
        }}
      />
    </>
  );
}
