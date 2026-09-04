import type { Issue, IssueExecutorType } from "@patchbay/core/types";

export type IssueExecutorRef = {
  type: IssueExecutorType;
  id: string;
};

/** Return only the issue's explicit execution role; ownership is independent. */
export function getIssueExecutor(
  issue: Pick<Issue, "executor_type" | "executor_id">,
): IssueExecutorRef | null {
  if (!issue.executor_type || !issue.executor_id) return null;
  return { type: issue.executor_type, id: issue.executor_id };
}
