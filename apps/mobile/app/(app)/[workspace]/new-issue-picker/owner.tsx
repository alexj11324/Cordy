import { router, Stack } from "expo-router";
import { RolePickerBody } from "@/components/issue/pickers/role-picker-body";
import { useAuthStore } from "@/data/auth-store";
import { useNewIssueDraftStore } from "@/data/stores/new-issue-draft-store";
import { getIssueRoleCopy } from "@/lib/issue-role-copy";
import { useNativeSearchBar } from "@/lib/use-native-search-bar";

export default function NewIssueOwnerPickerRoute() {
  const owner = useNewIssueDraftStore((state) => state.owner);
  const setOwner = useNewIssueDraftStore((state) => state.setOwner);
  const language = useAuthStore((state) => state.user?.language);
  const copy = getIssueRoleCopy(language);
  const query = useNativeSearchBar(copy.searchMembers, { autoFocus: true });

  return (
    <>
      <Stack.Screen options={{ title: copy.owner }} />
      <RolePickerBody
        kind="owner"
        value={owner}
        query={query}
        onChange={(next) => {
          setOwner(next);
          router.back();
        }}
      />
    </>
  );
}
