import { router, Stack } from "expo-router";
import { RolePickerBody } from "@/components/issue/pickers/role-picker-body";
import { useAuthStore } from "@/data/auth-store";
import { useNewIssueDraftStore } from "@/data/stores/new-issue-draft-store";
import { getIssueRoleCopy } from "@/lib/issue-role-copy";
import { useNativeSearchBar } from "@/lib/use-native-search-bar";

export default function NewIssueReviewerPickerRoute() {
  const reviewer = useNewIssueDraftStore((state) => state.reviewer);
  const setReviewer = useNewIssueDraftStore((state) => state.setReviewer);
  const language = useAuthStore((state) => state.user?.language);
  const copy = getIssueRoleCopy(language);
  const query = useNativeSearchBar(copy.searchReviewers, { autoFocus: true });

  return (
    <>
      <Stack.Screen options={{ title: copy.reviewer }} />
      <RolePickerBody
        kind="reviewer"
        value={reviewer}
        query={query}
        onChange={(next) => {
          setReviewer(next);
          router.back();
        }}
      />
    </>
  );
}
