import { useEffect, useRef, useState } from "react";
import { KeyboardAvoidingView, Platform, ScrollView, View } from "react-native";
import type { IssueStatus } from "@orvilo/core/types";
import { Button } from "@/components/ui/button";
import { Text } from "@/components/ui/text";
import { TextField } from "@/components/ui/text-field";
import { AutosizeTextArea } from "@/components/ui/autosize-textarea";
import { useAuthStore } from "@/data/auth-store";
import {
  buildReviewSubmissionPatch,
  type IssueRoleRef,
  type ReviewSubmissionPatch,
} from "@/lib/issue-review-workflow";
import { getIssueRoleCopy } from "@/lib/issue-role-copy";

export function ReviewSubmissionForm({
  status,
  reviewer,
  submitting,
  onSubmit,
}: {
  status: IssueStatus;
  reviewer: IssueRoleRef;
  submitting: boolean;
  onSubmit: (patch: ReviewSubmissionPatch) => void;
}) {
  const language = useAuthStore((state) => state.user?.language);
  const copy = getIssueRoleCopy(language);
  const writingRef = useRef(false);
  const [worktree, setWorktree] = useState("");
  const [branch, setBranch] = useState("");
  const [commit, setCommit] = useState("");
  const [pullRequests, setPullRequests] = useState("");
  const patch = buildReviewSubmissionPatch(status, reviewer, {
    worktree,
    branch,
    commit,
    pullRequests,
  });

  useEffect(() => {
    if (!submitting) writingRef.current = false;
  }, [submitting]);

  return (
    <KeyboardAvoidingView
      className="flex-1 bg-background"
      behavior={Platform.OS === "ios" ? "padding" : undefined}
    >
      <ScrollView
        className="flex-1"
        contentContainerClassName="gap-4 px-4 pb-8 pt-4"
        keyboardShouldPersistTaps="handled"
      >
        <View className="gap-1">
          <Text className="text-base font-semibold text-foreground">
            {copy.reviewHandoff}
          </Text>
          <Text className="text-sm leading-5 text-muted-foreground">
            {copy.reviewSubmissionDescription}
          </Text>
        </View>

        <EvidenceField label={copy.reviewWorktree}>
          <TextField
            value={worktree}
            onChangeText={setWorktree}
            placeholder={copy.reviewWorktreePlaceholder}
            autoCapitalize="none"
            autoCorrect={false}
            editable={!submitting}
          />
        </EvidenceField>

        <EvidenceField label={copy.reviewBranch}>
          <TextField
            value={branch}
            onChangeText={setBranch}
            placeholder={copy.reviewBranchPlaceholder}
            autoCapitalize="none"
            autoCorrect={false}
            editable={!submitting}
          />
        </EvidenceField>

        <EvidenceField label={copy.reviewCommit}>
          <TextField
            value={commit}
            onChangeText={setCommit}
            placeholder={copy.reviewCommitPlaceholder}
            autoCapitalize="none"
            autoCorrect={false}
            editable={!submitting}
          />
        </EvidenceField>

        <EvidenceField label={copy.reviewPullRequests}>
          <AutosizeTextArea
            value={pullRequests}
            onChangeText={setPullRequests}
            placeholder={copy.reviewPullRequestsPlaceholder}
            autoCapitalize="none"
            autoCorrect={false}
            keyboardType="url"
            editable={!submitting}
            minHeight={72}
            maxHeight={144}
            className="rounded-md border border-transparent bg-secondary/50 px-3 py-2"
          />
          <Text className="text-xs text-muted-foreground">
            {copy.reviewPullRequestsHint}
          </Text>
        </EvidenceField>

        <Button
          disabled={!patch || submitting}
          onPress={() => {
            if (!patch || writingRef.current) return;
            writingRef.current = true;
            onSubmit(patch);
          }}
        >
          <Text>{submitting ? copy.submittingReview : copy.submitReview}</Text>
        </Button>
      </ScrollView>
    </KeyboardAvoidingView>
  );
}

function EvidenceField({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}) {
  return (
    <View className="gap-1.5">
      <Text className="text-xs uppercase tracking-wider text-muted-foreground">
        {label}
      </Text>
      {children}
    </View>
  );
}
