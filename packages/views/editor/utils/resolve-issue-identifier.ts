import type { QueryClient } from "@tanstack/react-query";
import { issueIdentifierOptions } from "@patchbay/core/issues/queries";
import { workspaceListOptions } from "@patchbay/core/workspace/queries";
import { isIssueIdentifier } from "@patchbay/ui/markdown";
import type { ResolvedIssueRef } from "../extensions/issue-identifier-autolink";

/**
 * Resolve a bare identifier (`PB-123`) against the current workspace.
 *
 * Shared by Tiptap's live autolink plugin and the Agent Lexical composer so
 * both paths use the same prefix / exact-match rules.
 */
export async function resolveWorkspaceIssueIdentifier(
  queryClient: QueryClient,
  identifier: string,
  workspaceSlug: string | null | undefined,
): Promise<ResolvedIssueRef | null> {
  if (!isIssueIdentifier(identifier) || !workspaceSlug) return null;
  const workspaces = await queryClient.fetchQuery(workspaceListOptions());
  const ws = workspaces.find((workspace) => workspace.slug === workspaceSlug);
  if (!ws) return null;
  const prefix = ws.issue_prefix;
  if (
    prefix &&
    !identifier.toUpperCase().startsWith(`${prefix.toUpperCase()}-`)
  ) {
    return null;
  }
  const issue = await queryClient.fetchQuery(
    issueIdentifierOptions(ws.id, identifier),
  );
  return issue ? { id: issue.id, identifier: issue.identifier } : null;
}
