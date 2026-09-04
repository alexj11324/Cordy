/**
 * Status picker route for the in-progress new-issue draft. Reads/writes
 * `useNewIssueDraftStore` — the new-issue.tsx modal owns the draft and
 * reads from the same store. See ../new-issue.tsx for the lifecycle.
 */
import { Alert } from "react-native";
import { router, useLocalSearchParams } from "expo-router";
import { StatusPickerBody } from "@/components/issue/pickers/status-picker-body";
import { useAuthStore } from "@/data/auth-store";
import { useNewIssueDraftStore } from "@/data/stores/new-issue-draft-store";
import { getIssueRoleCopy } from "@/lib/issue-role-copy";
import { planIssueStatusSelection } from "@/lib/issue-review-workflow";
import { useIssueStatuses } from "@/lib/use-issue-statuses";

export default function NewIssueStatusPickerRoute() {
  const { workspace } = useLocalSearchParams<{ workspace: string }>();
  const status = useNewIssueDraftStore((s) => s.status);
  const setStatus = useNewIssueDraftStore((s) => s.setStatus);
  const executor = useNewIssueDraftStore((s) => s.executor);
  const reviewer = useNewIssueDraftStore((s) => s.reviewer);
  const language = useAuthStore((s) => s.user?.language);
  const copy = getIssueRoleCopy(language);
  const catalog = useIssueStatuses();

  return (
    <StatusPickerBody
      value={status}
      onChange={(next) => {
        const plan = planIssueStatusSelection({
          previousCategory: null,
          nextStatus: next,
          nextCategory: catalog.categoryOf(next),
          executor,
          reviewer,
        });
        if (plan.kind === "blocked") {
          Alert.alert(copy.executor, copy.executorRequired);
          return;
        }
        if (plan.kind === "choose_reviewer") {
          router.replace({
            pathname: "/[workspace]/new-issue-picker/reviewer",
            params: { workspace, handoffStatus: plan.status },
          });
          return;
        }
        setStatus(plan.status);
        router.back();
      }}
    />
  );
}
