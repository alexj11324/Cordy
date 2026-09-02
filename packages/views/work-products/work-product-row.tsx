"use client";

import {
  CheckCircle2,
  Circle,
  CircleDashed,
  CircleSlash,
  FileText,
  GitMerge,
  GitPullRequest,
  GitPullRequestArrow,
  GitPullRequestClosed,
  GitPullRequestDraft,
  TriangleAlert,
  X,
  XCircle,
} from "lucide-react";
import {
  deriveChecksStatus,
  deriveMergeStatus,
  shouldShowPullRequestStats,
  type PullRequestChecksStatus,
  type PullRequestMergeStatus,
} from "@patchbay/core/github";
import type {
  GitHubPullRequest,
  GitHubPullRequestState,
  WorkProductView,
} from "@patchbay/core/types";
import { useWorkspacePaths } from "@patchbay/core/paths";
import { Badge } from "@patchbay/ui/components/ui/badge";
import { cn } from "@patchbay/ui/lib/utils";
import { AppLink } from "../navigation";
import { useT, useTimeAgo } from "../i18n";

type IssuesT = ReturnType<typeof useT<"issues">>["t"];
type WorkProductsT = ReturnType<typeof useT<"work-products">>["t"];

// The pull-request labels are already translated in the `issues` namespace,
// where they lived when PRs had their own sidebar section. Keeping them there
// rather than copying twenty strings into `work-products` avoids four bundles
// of duplicate translations that would then have to drift in lockstep.

const STATE_ICON: Record<
  GitHubPullRequestState,
  { icon: React.ComponentType<{ className?: string }>; className: string }
> = {
  open: { icon: GitPullRequestArrow, className: "text-emerald-600 dark:text-emerald-400" },
  draft: { icon: GitPullRequestDraft, className: "text-muted-foreground" },
  merged: { icon: GitMerge, className: "text-violet-600 dark:text-violet-400" },
  closed: { icon: GitPullRequestClosed, className: "text-rose-600 dark:text-rose-400" },
};

/**
 * One row of the issue's Work Product list.
 *
 * A product that mirrors a pull request renders the same card the PR-only
 * sidebar used to: state icon, repo#number, CI and mergeability. Everything
 * else renders as a plain product. Both go through this component so the two
 * cannot drift into looking like different features again.
 */
export function WorkProductRow({
  product,
  onDetach,
  detachPending,
}: {
  product: WorkProductView;
  onDetach?: (relationId: string) => void;
  detachPending?: boolean;
}) {
  const { t } = useT("work-products");
  const paths = useWorkspacePaths();
  const pullRequest = product.pull_request;
  const cfg = pullRequest
    ? (STATE_ICON[pullRequest.state] ?? { icon: GitPullRequest, className: "" })
    : { icon: FileText, className: "text-muted-foreground" };
  const LeadIcon = cfg.icon;
  const isDraft = pullRequest?.state === "draft";
  const title = pullRequest?.title || product.external_identity || product.id;

  return (
    <div
      data-testid="work-product-row"
      className={cn(
        "group flex items-start gap-2 rounded-md -mx-2 px-2 py-1.5 transition-colors hover:bg-accent/50",
        isDraft ? "opacity-80" : null,
      )}
    >
      <LeadIcon aria-hidden="true" className={cn("mt-0.5 h-3.5 w-3.5 shrink-0", cfg.className)} />
      <div className="min-w-0 flex-1">
        {product.external_url ? (
          <a
            href={product.external_url}
            target="_blank"
            rel="noreferrer noopener"
            className="block truncate text-caption font-medium leading-snug hover:text-foreground"
          >
            {title}
          </a>
        ) : (
          <AppLink
            href={paths.workProductDetail(product.id)}
            className="block truncate text-caption font-medium leading-snug hover:text-foreground"
          >
            {title}
          </AppLink>
        )}
        <WorkProductSubtitle product={product} />
        {pullRequest ? <PullRequestDetails pr={pullRequest} /> : null}
      </div>
      {product.relation.close_intent ? (
        <Badge variant="secondary">{t(($) => $.relations.close_intent_short)}</Badge>
      ) : null}
      {onDetach ? (
        <button
          type="button"
          disabled={detachPending}
          aria-label={t(($) => $.relations.detach)}
          title={t(($) => $.relations.detach)}
          onClick={() => onDetach(product.relation.id)}
          className="shrink-0 rounded p-1 text-muted-foreground opacity-0 transition-opacity hover:bg-accent hover:text-foreground focus-visible:opacity-100 group-hover:opacity-100 disabled:opacity-50"
        >
          <X aria-hidden="true" className="h-3 w-3" />
        </button>
      ) : null}
    </div>
  );
}

// The subtitle answers "what is this and why is it here". For a PR the first
// half is the provider's own coordinates; for anything else it is the product
// identity. The relation source is always shown, because a product attached by
// a webhook and one a person picked deserve different trust.
function WorkProductSubtitle({ product }: { product: WorkProductView }) {
  const { t: tIssues } = useT("issues");
  const { t } = useT("work-products");
  const pr = product.pull_request;
  const head = pr
    ? `${pr.repo_owner}/${pr.repo_name}#${pr.number} · ${getStateLabel(pr.state, tIssues)}${
        pr.author_login ? ` · @${pr.author_login}` : ""
      }`
    : `${product.provider} · ${product.kind}`;
  return (
    <p className="truncate text-micro text-muted-foreground">
      {head} · {relationSourceLabel(product.relation.relation_source, t)}
    </p>
  );
}

function relationSourceLabel(source: string, t: WorkProductsT): string {
  switch (source) {
    case "manual_explicit":
      return t(($) => $.relations.source_manual);
    case "task_explicit":
      return t(($) => $.relations.source_task);
    case "execution_branch_discovery":
      return t(($) => $.relations.source_branch);
    case "provider_discovery":
      return t(($) => $.relations.source_provider);
    default:
      return source;
  }
}

function PullRequestDetails({ pr }: { pr: GitHubPullRequest }) {
  const { t } = useT("issues");
  const timeAgo = useTimeAgo();

  const showStats = shouldShowPullRequestStats({
    additions: pr.additions,
    deletions: pr.deletions,
    changed_files: pr.changed_files,
  });

  // Neither status element is shown for terminal PRs — the leading state icon
  // already conveys merged / closed, and CI / mergeability are no longer
  // actionable there.
  const isTerminal = pr.state === "merged" || pr.state === "closed";
  const checksBadge = isTerminal ? null : getChecksBadge(deriveChecksStatus(pr), t);
  const mergeBadge = isTerminal ? null : getMergeBadge(deriveMergeStatus(pr), t);

  // A stale snapshot (GitHub outage / revoked key) greys out both elements and
  // annotates them with the snapshot age instead of hiding the last-known data.
  const stale = !isTerminal && pr.snapshot_stale === true;
  const staleTitle = stale
    ? pr.snapshot_fetched_at
      ? t(($) => $.detail.pull_request_snapshot_stale, { time: timeAgo(pr.snapshot_fetched_at) })
      : t(($) => $.detail.pull_request_snapshot_stale_unknown)
    : undefined;

  if (!showStats && !checksBadge && !mergeBadge) return null;

  return (
    <div className="mt-1 flex flex-wrap items-center gap-x-2 gap-y-0.5 text-micro text-muted-foreground">
      {showStats ? <PullRequestStats pr={pr} /> : null}
      {checksBadge ? <PullRequestBadge badge={checksBadge} stale={stale} title={staleTitle} /> : null}
      {mergeBadge ? <PullRequestBadge badge={mergeBadge} stale={stale} title={staleTitle} /> : null}
    </div>
  );
}

function PullRequestStats({ pr }: { pr: GitHubPullRequest }) {
  const { t } = useT("issues");
  return (
    <span className="inline-flex items-center gap-1.5 tabular-nums">
      <span className="text-emerald-600 dark:text-emerald-400">+{pr.additions ?? 0}</span>
      <span className="text-rose-600 dark:text-rose-400">−{pr.deletions ?? 0}</span>
      <span aria-hidden="true">·</span>
      <span>
        {t(($) => $.detail.pull_request_card_files_count, {
          count: pr.changed_files ?? 0,
        })}
      </span>
    </span>
  );
}

interface PullRequestBadgeConfig {
  icon: React.ComponentType<{ className?: string }>;
  label: string;
  className: string;
}

function PullRequestBadge({
  badge,
  stale,
  title,
}: {
  badge: PullRequestBadgeConfig;
  stale?: boolean;
  title?: string;
}) {
  const Icon = badge.icon;
  return (
    <span
      className={cn("inline-flex items-center gap-1", stale ? "opacity-60" : null)}
      title={title}
    >
      <Icon className={cn("h-3 w-3", badge.className)} />
      {badge.label}
    </span>
  );
}

// CI element. A current snapshot with a null rollup renders "no checks yet";
// an unavailable/disabled snapshot renders nothing.
function getChecksBadge(
  status: PullRequestChecksStatus,
  t: IssuesT,
): PullRequestBadgeConfig | null {
  switch (status.kind) {
    case "failed":
      return {
        icon: XCircle,
        className: "text-rose-600 dark:text-rose-400",
        label: checksFailedLabel(status, t),
      };
    case "pending":
      return {
        icon: CircleDashed,
        className: "text-amber-600 dark:text-amber-400",
        label: t(($) => $.detail.pull_request_checks_running, {
          passed: status.passed,
          total: status.total,
          running: status.running,
        }),
      };
    case "passed":
      return {
        icon: CheckCircle2,
        className: "text-emerald-600 dark:text-emerald-400",
        label: t(($) => $.detail.pull_request_checks_all_passed, { total: status.total }),
      };
    case "none":
      return {
        icon: Circle,
        className: "text-muted-foreground",
        label: t(($) => $.detail.pull_request_checks_none),
      };
    case "unavailable":
      return null;
  }
}

function checksFailedLabel(
  status: Extract<PullRequestChecksStatus, { kind: "failed" }>,
  t: IssuesT,
): string {
  const shown = status.names.slice(0, 2);
  if (shown.length === 0) {
    return t(($) => $.detail.pull_request_checks_failed_count, {
      failed: status.failed,
      total: status.total,
    });
  }
  const remaining = status.names.length - shown.length;
  const parts = [...shown];
  if (remaining > 0) {
    parts.push(t(($) => $.detail.pull_request_checks_more, { count: remaining }));
  }
  return t(($) => $.detail.pull_request_checks_failed_named, {
    failed: status.failed,
    total: status.total,
    names: parts.join(", "),
  });
}

// Mergeability element. Returns null for the "none" state — when GitHub has not
// decided, the card asserts neither "conflict" nor "ready".
function getMergeBadge(status: PullRequestMergeStatus, t: IssuesT): PullRequestBadgeConfig | null {
  switch (status.kind) {
    case "conflicting":
      return {
        icon: TriangleAlert,
        className: "text-amber-600 dark:text-amber-400",
        label: t(($) => $.detail.pull_request_merge_conflicting),
      };
    case "ready":
      return {
        icon: CheckCircle2,
        className: "text-emerald-600 dark:text-emerald-400",
        label: t(($) => $.detail.pull_request_merge_ready),
      };
    case "blocked":
      return {
        icon: CircleSlash,
        className: "text-muted-foreground",
        label: t(($) => $.detail.pull_request_merge_blocked),
      };
    case "behind":
      return {
        icon: CircleSlash,
        className: "text-muted-foreground",
        label: t(($) => $.detail.pull_request_merge_behind),
      };
    case "unstable":
      return {
        icon: CircleSlash,
        className: "text-muted-foreground",
        label: t(($) => $.detail.pull_request_merge_unstable),
      };
    case "has_hooks":
      return {
        icon: CircleSlash,
        className: "text-muted-foreground",
        label: t(($) => $.detail.pull_request_merge_has_hooks),
      };
    case "none":
      return null;
  }
}

function getStateLabel(state: GitHubPullRequestState, t: IssuesT): string {
  return state === "open"
    ? t(($) => $.detail.pull_request_state_open)
    : state === "draft"
      ? t(($) => $.detail.pull_request_state_draft)
      : state === "merged"
        ? t(($) => $.detail.pull_request_state_merged)
        : state === "closed"
          ? t(($) => $.detail.pull_request_state_closed)
          : state;
}
