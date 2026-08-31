/**
 * Executor picker route for the in-progress new-issue draft. See ./status.tsx.
 * Uses the same iOS-native nav header + UISearchController pattern as
 * `issue/[id]/picker/executor.tsx`, with the search bar wiring encapsulated
 * in `useNativeSearchBar`.
 */
import { router } from "expo-router";
import { ExecutorPickerBody } from "@/components/issue/pickers/executor-picker-body";
import { useNewIssueDraftStore } from "@/data/stores/new-issue-draft-store";
import { useNativeSearchBar } from "@/lib/use-native-search-bar";

export default function NewIssueExecutorPickerRoute() {
  const executor = useNewIssueDraftStore((s) => s.executor);
  const setExecutor = useNewIssueDraftStore((s) => s.setExecutor);
  const query = useNativeSearchBar("Search people", { autoFocus: true });

  return (
    <ExecutorPickerBody
      value={executor}
      query={query}
      onChange={(next) => {
        setExecutor(next);
        router.back();
      }}
    />
  );
}
