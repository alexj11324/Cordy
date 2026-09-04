import { useRef } from "react";
import { ActivityIndicator, Alert, View } from "react-native";
import { router, Stack, useLocalSearchParams } from "expo-router";
import { useQuery } from "@tanstack/react-query";
import { RolePickerBody } from "@/components/issue/pickers/role-picker-body";
import { Button } from "@/components/ui/button";
import { Text } from "@/components/ui/text";
import { useAuthStore } from "@/data/auth-store";
import { useUpdateIssue } from "@/data/mutations/issues";
import { issueDetailOptions } from "@/data/queries/issues";
import { useWorkspaceStore } from "@/data/workspace-store";
import { getIssueRoleCopy } from "@/lib/issue-role-copy";
import { reviewerPatch } from "@/lib/issue-role-patch";
import { issueActorForRole } from "@/lib/issue-scope";
import { issueStatusCategory } from "@/lib/issue-status";
import {
  isReviewHandoff,
  reviewHandoffPatch,
} from "@/lib/issue-review-workflow";
import { useNativeSearchBar } from "@/lib/use-native-search-bar";
import { useIssueStatuses } from "@/lib/use-issue-statuses";

export default function IssueReviewerPickerRoute() {
  const { id, handoffStatus } = useLocalSearchParams<{
    id: string;
    handoffStatus?: string;
  }>();
  const wsId = useWorkspaceStore((state) => state.currentWorkspaceId);
  const language = useAuthStore((state) => state.user?.language);
  const copy = getIssueRoleCopy(language);
  const catalog = useIssueStatuses();
  const detail = useQuery(issueDetailOptions(wsId, id));
  const issue = detail.data;
  const updateIssue = useUpdateIssue(id);
  const writingRef = useRef(false);
  const query = useNativeSearchBar(copy.searchReviewers, { autoFocus: true });
  const value = issue ? issueActorForRole(issue, "reviewer") : null;
  const executor = issue ? issueActorForRole(issue, "executor") : null;
  const currentCategory = issue
    ? (issueStatusCategory(issue) ?? catalog.categoryOf(issue.status))
    : null;
  const nextCategory = handoffStatus
    ? catalog.categoryOf(handoffStatus)
    : currentCategory;
  const isHandoff =
    !!handoffStatus &&
    !!nextCategory &&
    isReviewHandoff(currentCategory, nextCategory);

  if (detail.isLoading) {
    return (
      <>
        <Stack.Screen
          options={{ title: isHandoff ? copy.reviewHandoff : copy.reviewer }}
        />
        <View className="flex-1 items-center justify-center bg-background">
          <ActivityIndicator />
        </View>
      </>
    );
  }
  if (detail.error || !issue) {
    return (
      <>
        <Stack.Screen
          options={{ title: isHandoff ? copy.reviewHandoff : copy.reviewer }}
        />
        <View className="flex-1 items-center justify-center gap-3 bg-background px-6">
          <Text className="text-sm text-destructive">
            {copy.loadIssueFailed}
          </Text>
          <Button variant="outline" onPress={() => detail.refetch()}>
            <Text>{copy.retry}</Text>
          </Button>
        </View>
      </>
    );
  }

  return (
    <>
      <Stack.Screen
        options={{ title: isHandoff ? copy.reviewHandoff : copy.reviewer }}
      />
      <RolePickerBody
        kind="reviewer"
        value={value}
        query={query}
        disabled={updateIssue.isPending}
        allowUnassigned={!isHandoff}
        excludedActor={
          isHandoff || currentCategory === "in_review" ? executor : null
        }
        onChange={(next) => {
          if (writingRef.current || (isHandoff && !next)) return;
          writingRef.current = true;
          updateIssue.mutate(
            isHandoff && handoffStatus && next
              ? reviewHandoffPatch(handoffStatus, next)
              : reviewerPatch(next),
            {
              onSuccess: () => router.back(),
              onError: (error) =>
                Alert.alert(
                  copy.updateFailed,
                  error instanceof Error ? error.message : copy.updateFailed,
                ),
              onSettled: () => {
                writingRef.current = false;
              },
            },
          );
        }}
      />
    </>
  );
}
