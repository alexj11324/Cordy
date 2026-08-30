"use client";

import { useState, type FormEvent } from "react";
import { useQuery } from "@tanstack/react-query";
import { toast } from "sonner";
import {
  CheckCircle2,
  Circle,
  CircleDashed,
  CircleSlash,
  GitMerge,
  GitPullRequest,
  GitPullRequestArrow,
  GitPullRequestClosed,
  GitPullRequestDraft,
  TriangleAlert,
  XCircle,
} from "lucide-react";
import {
  issuePullRequestsOptions,
  unassociatedWorkProductsOptions,
  deriveChecksStatus,
  deriveMergeStatus,
  shouldShowPullRequestStats,
  useAttachIssuePullRequest,
  useAttachIssueWorkProduct,
  type PullRequestChecksStatus,
  type PullRequestMergeStatus,
} from "@patchbay/core/github";
import type { GitHubPullRequest, GitHubPullRequestState } from "@patchbay/core/types";
import { Button } from "@patchbay/ui/components/ui/button";
import { cn } from "@patchbay/ui/lib/utils";
import { useT, useTimeAgo } from "../../i18n";

type IssuesT = ReturnType<typeof useT<"issues">>["t"];

// Keep the existing sidebar density: show the first 3 PR rows inline, then
// collapse the rest once the section reaches 4 rows.
const PR_LIMIT_BEFORE_COLLAPSE = 4;

const STATE_ICON: Record<
  GitHubPullRequestState,
  { icon: React.ComponentType<{ className?: string }>; className: string }
> = {
  open: { icon: GitPullRequestArrow, className: "text-emerald-600 dark:text-emerald-400" },
  draft: { icon: GitPullRequestDraft, className: "text-muted-foreground" },
  merged: { icon: GitMerge, className: "text-violet-600 dark:text-violet-400" },
  closed: { icon: GitPullRequestClosed, className: "text-rose-600 dark:text-rose-400" },
};

export function PullRequestList({ issueId, workspaceId }: { issueId: string; workspaceId: string }) {
  const { t } = useT("issues");
  const [expanded, setExpanded] = useState(false);
  const { data, isLoading } = useQuery(issuePullRequestsOptions(issueId));
  const attachMutation = useAttachIssuePullRequest(issueId, workspaceId);
  const prs = data?.pull_requests ?? [];

  if (isLoading) {
    return <p className="text-caption text-muted-foreground px-2">{t(($) => $.detail.pull_requests_loading)}</p>;
  }
  if (prs.length === 0) {
    return (
      <div className="space-y-2">
        <p className="text-caption text-muted-foreground px-2">
          {t(($) => $.detail.pull_requests_empty)}
        </p>
        <UnassociatedPullRequests issueId={issueId} workspaceId={workspaceId} />
        <AttachPullRequestForm issueId={issueId} mutation={attachMutation} />
      </div>
    );
  }

  // Render rule:
  //   - <  PR_LIMIT_BEFORE_COLLAPSE: every PR row is visible.
  //   - >= PR_LIMIT_BEFORE_COLLAPSE: first (LIMIT - 1) rows are visible and
  //     the remainder sits behind a toggle.
  const useCollapse = prs.length >= PR_LIMIT_BEFORE_COLLAPSE;
  const expandedHead = useCollapse ? prs.slice(0, PR_LIMIT_BEFORE_COLLAPSE - 1) : prs;
  const collapsedTail = useCollapse ? prs.slice(PR_LIMIT_BEFORE_COLLAPSE - 1) : [];

  return (
    <div className="space-y-1">
      {expandedHead.map((pr) => (
        <PullRequestRow key={pr.id} pr={pr} />
      ))}
      {useCollapse ? (
        <div className="space-y-1">
          {expanded
            ? collapsedTail.map((pr) => <PullRequestRow key={pr.id} pr={pr} />)
            : null}
          <button
            type="button"
            onClick={() => setExpanded((v) => !v)}
            className="block w-[calc(100%+1rem)] -mx-2 rounded-md px-2 py-1.5 text-left text-micro text-muted-foreground hover:bg-accent/50 hover:text-foreground transition-colors"
          >
            {expanded
              ? t(($) => $.detail.pull_request_card_show_less)
              : t(($) => $.detail.pull_request_card_show_more, { count: collapsedTail.length })}
          </button>
        </div>
      ) : null}
      <UnassociatedPullRequests issueId={issueId} workspaceId={workspaceId} />
      <AttachPullRequestForm issueId={issueId} mutation={attachMutation} />
    </div>
  );
}

function UnassociatedPullRequests({ issueId, workspaceId }: { issueId: string; workspaceId: string }) {
  const { t } = useT("issues");
  const { data } = useQuery(unassociatedWorkProductsOptions(workspaceId));
  const mutation = useAttachIssueWorkProduct(issueId, workspaceId);
  const [attachingId, setAttachingId] = useState<string | null>(null);
  const products = (data?.work_products ?? []).filter(
    (product) =>
      product.kind === "pull_request" &&
      product.external_url,
  );

  if (products.length === 0) return null;

  return (
    <div
      data-testid="unassociated-pull-requests"
      className="space-y-1 rounded-md border border-dashed border-border/70 px-2 py-1.5"
    >
      <p className="text-micro font-medium text-muted-foreground">
        {t(($) => $.detail.pull_requests_unassociated_heading)}
      </p>
      {products.map((product) => (
        <div key={product.id} className="flex items-center gap-2 text-micro">
          <a
            href={product.external_url ?? undefined}
            target="_blank"
            rel="noreferrer noopener"
            className="min-w-0 flex-1 truncate text-muted-foreground hover:text-foreground"
          >
            {product.external_identity}
          </a>
          <Button
            type="button"
            size="sm"
            variant="ghost"
            disabled={mutation.isPending}
            onClick={() => {
              setAttachingId(product.id);
              mutation.mutate(
                { work_product_id: product.id },
                {
                  onSuccess: () => toast.success(t(($) => $.detail.pull_requests_attach_success)),
                  onError: () => toast.error(t(($) => $.detail.pull_requests_attach_failed)),
                  onSettled: () => setAttachingId(null),
                },
              );
            }}
          >
            {mutation.isPending && attachingId === product.id
              ? t(($) => $.detail.pull_requests_attaching)
              : t(($) => $.detail.pull_requests_unassociated_attach)}
          </Button>
        </div>
      ))}
    </div>
  );
}

function AttachPullRequestForm({
  issueId,
  mutation,
}: {
  issueId: string;
  mutation: ReturnType<typeof useAttachIssuePullRequest>;
}) {
  const { t } = useT("issues");
  const [url, setUrl] = useState("");

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const value = url.trim();
    if (!value || mutation.isPending) return;
    mutation.mutate(
      { url: value },
      {
        onSuccess: () => {
          setUrl("");
          toast.success(t(($) => $.detail.pull_requests_attach_success));
        },
        onError: () => {
          toast.error(t(($) => $.detail.pull_requests_attach_failed));
        },
      },
    );
  }

  return (
    <form
      data-testid="attach-pull-request-form"
      onSubmit={submit}
      className="flex items-center gap-2 px-2 pt-1"
    >
      <label htmlFor={`attach-pull-request-${issueId}`} className="sr-only">
        {t(($) => $.detail.pull_requests_attach_label)}
      </label>
      <input
        id={`attach-pull-request-${issueId}`}
        data-testid="attach-pull-request-input"
        type="url"
        value={url}
        onChange={(event) => setUrl(event.target.value)}
        placeholder={t(($) => $.detail.pull_requests_attach_placeholder)}
        aria-label={t(($) => $.detail.pull_requests_attach_label)}
        disabled={mutation.isPending}
        className="min-w-0 flex-1 rounded-md border border-input bg-background px-2 py-1 text-micro outline-none placeholder:text-muted-foreground focus-visible:ring-1 focus-visible:ring-ring"
      />
      <Button
        data-testid="attach-pull-request-button"
        type="submit"
        size="sm"
        variant="outline"
        disabled={!url.trim() || mutation.isPending}
      >
        {mutation.isPending
          ? t(($) => $.detail.pull_requests_attaching)
          : t(($) => $.detail.pull_requests_attach_action)}
      </Button>
    </form>
  );
}

function PullRequestRow({ pr }: { pr: GitHubPullRequest }) {
  const { t } = useT("issues");
  const cfg = STATE_ICON[pr.state] ?? { icon: GitPullRequest, className: "" };
  const StateIcon = cfg.icon;
  const isDraft = pr.state === "draft";
  const stateLabel = getStateLabel(pr.state, t);

  return (
    <a
      data-testid="pull-request-row"
      href={pr.html_url}
      target="_blank"
      rel="noreferrer noopener"
      className={cn(
        "flex items-start gap-2 rounded-md px-2 py-1.5 -mx-2 hover:bg-accent/50 transition-colors group",
        isDraft ? "opacity-80" : null,
      )}
    >
      <StateIcon className={cn("h-3.5 w-3.5 mt-0.5 shrink-0", cfg.className)} />
      <div className="min-w-0 flex-1">
        <p className="text-caption font-medium leading-snug truncate group-hover:text-foreground">
          {pr.title}
        </p>
        <p className="text-micro text-muted-foreground truncate">
          {pr.repo_owner}/{pr.repo_name}#{pr.number} · {stateLabel}
          {pr.author_login ? ` · @${pr.author_login}` : null}
        </p>
        <PullRequestRowDetails pr={pr} />
      </div>
    </a>
  );
}

function PullRequestRowDetails({ pr }: { pr: GitHubPullRequest }) {
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
