"use client";

import { useCallback, useMemo } from "react";
import { toast } from "sonner";
import type { UpdateIssueRequest } from "@orvilo/core/types";
import {
  useBatchDeleteIssues,
  useBatchUpdateIssues,
  useUpdateIssue,
} from "@orvilo/core/issues/mutations";
import { errorCode } from "@orvilo/core/api";
import { useModalStore } from "@orvilo/core/modals";
import {
  type IssueSurfaceActions,
  type IssueSurfaceMutationOptions,
} from "./actions-context";
import type { IssueCreateDefaults } from "./types";
import { useT } from "../../i18n";

export type MoveIssueUpdates = Pick<
  UpdateIssueRequest,
  | "status"
  | "executor_type"
  | "executor_id"
  | "position"
  | "parent_issue_id"
  | "project_id"
> & {
  before_id: string | null;
  after_id: string | null;
};

export type MoveIssueCallbacks = {
  onSettled?: () => void;
  onSuccess?: () => void;
  onError?: () => void;
};

export interface IssueSurfaceActionController {
  actions: IssueSurfaceActions;
  openCreateIssue: (defaults?: IssueCreateDefaults) => void;
  moveIssue: (
    issueId: string,
    updates: MoveIssueUpdates,
    callbacks?: MoveIssueCallbacks,
  ) => void;
}

export function useIssueSurfaceActions({
  createDefaults,
}: {
  createDefaults: IssueCreateDefaults;
}): IssueSurfaceActionController {
  const { t } = useT("projects");
  const { t: tIssues } = useT("issues");
  const updateIssueMutation = useUpdateIssue();
  const batchUpdateMutation = useBatchUpdateIssues();
  const batchDeleteMutation = useBatchDeleteIssues();

  const updateIssue = useCallback(
    (
      issueId: string,
      updates: Partial<UpdateIssueRequest>,
      options?: IssueSurfaceMutationOptions,
    ) => {
      updateIssueMutation.mutate(
        { id: issueId, ...updates },
        {
          onSuccess: (issue) => options?.onSuccess?.(issue),
          onError: (err) => {
            toast.error(
              errorCode(err) === "revision_conflict"
                ? tIssues(($) => $.revision.conflict)
                : err instanceof Error && err.message
                ? err.message
                : (options?.errorMessage ??
                    t(($) => $.detail.toast_move_issue_failed)),
            );
            options?.onError?.(err);
          },
          onSettled: () => options?.onSettled?.(),
        },
      );
    },
    [t, tIssues, updateIssueMutation],
  );

  const moveIssue = useCallback(
    (
      issueId: string,
      updates: MoveIssueUpdates,
      callbacks?: MoveIssueCallbacks,
    ) => {
      const { before_id, after_id, ...optimisticUpdates } = updates;
      // The promise belongs to this move even if navigation unmounts its
      // observer or a subsequent move replaces it. Per-call mutation options
      // are discarded in those cases and cannot restore persisted visibility.
      void updateIssueMutation
        .mutateAsync({
          id: issueId,
          ...optimisticUpdates,
          move_intent: { before_id, after_id },
        })
        .then(
          () => callbacks?.onSuccess?.(),
          (err) => {
            toast.error(
              errorCode(err) === "revision_conflict"
                ? tIssues(($) => $.revision.conflict)
                : err instanceof Error && err.message
                  ? err.message
                  : t(($) => $.detail.toast_move_issue_failed),
            );
            callbacks?.onError?.();
          },
        )
        .finally(() => callbacks?.onSettled?.());
    },
    [t, tIssues, updateIssueMutation],
  );

  const updateIssueAsync = useCallback(
    async (
      issueId: string,
      updates: Partial<UpdateIssueRequest>,
      options?: IssueSurfaceMutationOptions,
    ) => {
      try {
        const issue = await updateIssueMutation.mutateAsync({
          id: issueId,
          ...updates,
        });
        options?.onSuccess?.(issue);
        return issue;
      } catch (err) {
        toast.error(
          errorCode(err) === "revision_conflict"
            ? tIssues(($) => $.revision.conflict)
            : err instanceof Error && err.message
              ? err.message
              : (options?.errorMessage ??
                t(($) => $.detail.toast_move_issue_failed)),
        );
        options?.onError?.(err);
        throw err;
      } finally {
        options?.onSettled?.();
      }
    },
    [t, tIssues, updateIssueMutation],
  );

  const openCreateIssue = useCallback(
    (defaults?: IssueCreateDefaults) => {
      useModalStore
        .getState()
        .open("create-issue", { ...createDefaults, ...defaults });
    },
    [createDefaults],
  );

  const actions = useMemo<IssueSurfaceActions>(
    () => ({
      isPending:
        updateIssueMutation.isPending ||
        batchUpdateMutation.isPending ||
        batchDeleteMutation.isPending,
      createIssue: openCreateIssue,
      updateIssue,
      updateIssueAsync,
      moveIssue: (issueId, updates, options) =>
        updateIssue(issueId, updates, {
          errorMessage: t(($) => $.detail.toast_move_issue_failed),
          ...options,
        }),
      batchUpdate: async (issueIds, updates) => {
        const result = await batchUpdateMutation.mutateAsync({
          ids: issueIds,
          updates,
        });
        if (result.updated !== issueIds.length) {
          throw new Error(tIssues(($) => $.batch.update_failed));
        }
      },
      batchDelete: async (issueIds) => {
        await batchDeleteMutation.mutateAsync(issueIds);
      },
    }),
    [
      batchDeleteMutation,
      batchUpdateMutation,
      openCreateIssue,
      t,
      tIssues,
      updateIssue,
      updateIssueAsync,
      updateIssueMutation.isPending,
    ],
  );

  return { actions, openCreateIssue, moveIssue };
}
