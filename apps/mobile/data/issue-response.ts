import type { Issue } from "@patchbay/core/types";
import {
  CreateIssueResponseSchema,
  IssueSchema,
} from "@patchbay/core/api/schemas";
import { parseWithFallback } from "@/lib/parse-response";

export function parseCreatedIssueResponse(raw: unknown): Issue {
  const issue = parseWithFallback<Issue | null>(
    raw,
    CreateIssueResponseSchema,
    null,
    { endpoint: "POST /api/issues" },
  );
  if (!issue) throw new Error("Invalid issue create response");
  return issue;
}

export function parseUpdatedIssueResponse(
  expectedIssueId: string,
  raw: unknown,
): Issue {
  const issue = parseWithFallback<Issue | null>(raw, IssueSchema, null, {
    endpoint: "PUT /api/issues/:id",
  });
  if (!issue || issue.id !== expectedIssueId) {
    throw new Error("Invalid issue update response");
  }
  return issue;
}
