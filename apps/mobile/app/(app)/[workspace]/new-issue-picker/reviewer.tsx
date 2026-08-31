import { router } from "expo-router";
import { RolePickerBody } from "@/components/issue/pickers/role-picker-body";
import { useNewIssueDraftStore } from "@/data/stores/new-issue-draft-store";
import { useNativeSearchBar } from "@/lib/use-native-search-bar";

export default function NewIssueReviewerPickerRoute() {
  const reviewer = useNewIssueDraftStore((s) => s.reviewer);
  const setReviewer = useNewIssueDraftStore((s) => s.setReviewer);
  const query = useNativeSearchBar("Search reviewers", { autoFocus: true });
  return (
    <RolePickerBody
      kind="reviewer"
      value={reviewer}
      query={query}
      onChange={(next) => {
        setReviewer(next);
        router.back();
      }}
    />
  );
}
