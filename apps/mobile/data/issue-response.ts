import type { Issue } from "@patchbay/core/types";

/** Require the identity fields after ApiClient's schema-validation rail. */
export function requireCreatedIssueResponse(issue: Issue | null): Issue {
  if (!issue) throw new Error("Invalid issue create response");
  return issue;
}

export function requireUpdatedIssueResponse(
  expectedIssueId: string,
  issue: Issue | null,
): Issue {
  if (!issue || issue.id !== expectedIssueId) {
    throw new Error("Invalid issue update response");
  }
  return issue;
}
