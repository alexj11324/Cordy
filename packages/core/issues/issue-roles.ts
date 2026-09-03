import type { Issue, IssueExecutorType } from "../types/issue";

export type IssueExecutorRef = { type: IssueExecutorType; id: string };

type IssueExecutorFields = Pick<Issue, "executor_type" | "executor_id">;

/** The issue's execution target, when both halves of the role pair are present. */
export function issueExecutorRef(issue: IssueExecutorFields): IssueExecutorRef | null {
  if (issue.executor_type && issue.executor_id) {
    return { type: issue.executor_type, id: issue.executor_id };
  }
  return null;
}
