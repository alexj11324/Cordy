import { router } from "expo-router";
import { RolePickerBody } from "@/components/issue/pickers/role-picker-body";
import { useNewIssueDraftStore } from "@/data/stores/new-issue-draft-store";
import { useNativeSearchBar } from "@/lib/use-native-search-bar";

export default function NewIssueOwnerPickerRoute() {
  const owner = useNewIssueDraftStore((s) => s.owner);
  const setOwner = useNewIssueDraftStore((s) => s.setOwner);
  const query = useNativeSearchBar("Search members", { autoFocus: true });
  return (
    <RolePickerBody
      kind="owner"
      value={owner}
      query={query}
      onChange={(next) => {
        setOwner(next);
        router.back();
      }}
    />
  );
}
