"use client";

import { useEffect, useRef } from "react";
import { useCanonicalIssue } from "@patchbay/core/issues/canonical-id";
import { useWorkspaceId } from "@patchbay/core/hooks";
import { useWorkspacePaths } from "@patchbay/core/paths";
import { useNavigation } from "../../navigation";
import { IssueDetail, IssueDetailSkeleton, IssueNotFound } from "./issue-detail";

interface IssueDetailRouteProps {
  /**
   * Raw `/{ws}/issues/{segment}` parameter. Either a UUID (older links, and
   * anything the app itself linked before the issue row was in hand) or a
   * human-readable identifier such as `PB-123`.
   */
  routeId: string;
  /** Suppress live-task write controls for a read-only host. */
  readOnly?: boolean;
  onDelete?: () => void;
}

/**
 * Rewrite `/{ws}/issues/{uuid}` to `/{ws}/issues/{identifier}` once the issue
 * is known, so the address bar and any copied URL read as `PB-123`.
 *
 * A replace, not a push: the UUID URL is the same page, and a history entry
 * for it would make Back bounce the user between two spellings of one issue.
 */
export function useCanonicalIssueUrl(routeId: string, identifier: string | undefined) {
  const paths = useWorkspacePaths();
  const navigation = useNavigation();
  const canonicalHref = identifier ? paths.issueDetail(identifier) : null;
  // `useWorkspacePaths()` and the navigation adapter are both rebuilt on
  // render, so this ref — not the dependency array — is what guarantees the
  // replace runs once per target instead of on every commit.
  const replacedRef = useRef<string | null>(null);

  useEffect(() => {
    if (!canonicalHref || routeId === identifier) return;
    if (replacedRef.current === canonicalHref) return;
    replacedRef.current = canonicalHref;
    navigation.replace(canonicalHref);
  }, [canonicalHref, identifier, routeId, navigation]);
}

/**
 * Route-level wrapper around `IssueDetail` for `/{ws}/issues/{id}`.
 *
 * Two jobs the panel-embedded `IssueDetail` must not do:
 *  - resolve an identifier segment to the issue's UUID before rendering, so
 *    every cache key below stays UUID-keyed and realtime updates land (see
 *    `useCanonicalIssue`);
 *  - rewrite the address bar to the canonical identifier URL. That belongs to
 *    the route and only the route — the inbox renders `IssueDetail` in a side
 *    panel, where replacing the URL would navigate the user out of the inbox.
 */
export function IssueDetailRoute({ routeId, readOnly = false, onDelete }: IssueDetailRouteProps) {
  const wsId = useWorkspaceId();
  const { canonicalId, issue, isResolving, notFound } = useCanonicalIssue(wsId, routeId);

  useCanonicalIssueUrl(routeId, issue?.identifier);

  if (isResolving) return <IssueDetailSkeleton />;

  // Render not-found here rather than handing the unresolved segment down.
  // `IssueDetail` would mount a second observer on the query that just failed,
  // refetch it, and restart this component's resolve/remount cycle — an
  // unbounded request loop that never settles. See `CanonicalIssue.notFound`.
  if (notFound || !canonicalId) return <IssueNotFound showBackLink={!onDelete} />;

  return <IssueDetail issueId={canonicalId} readOnly={readOnly} onDelete={onDelete} />;
}
