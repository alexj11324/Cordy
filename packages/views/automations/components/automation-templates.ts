import type { WebhookEventFilter } from "@patchbay/core/types";
import type { ScheduleConfig } from "./schedule-editor/model";

export const TEMPLATE_CATEGORY_IDS = [
  "popular",
  "code_review",
  "security",
  "incidents_triage",
  "data_research",
  "environment",
] as const;

export type TemplateCategoryId = (typeof TEMPLATE_CATEGORY_IDS)[number];

export const TEMPLATE_TRIGGER_IDS = [
  "scheduled",
  "pr_opened",
  "pr_pushed",
  "pr_review_comment",
  "new_message_in_channel",
  "workflow_run_completed",
  "checks_completed",
  "incident_triggered",
  "sentry_issue_event",
  "issue_created",
] as const;

export type TemplateTriggerId = (typeof TEMPLATE_TRIGGER_IDS)[number];

export const TEMPLATE_ACTION_IDS = [
  "send_slack",
  "pr_comment",
  "request_reviewers",
  "granola",
  "databricks_sql",
  "notion",
  "stripe",
  "slack",
  "datadog",
] as const;

export type TemplateActionId = (typeof TEMPLATE_ACTION_IDS)[number];

export const AUTOMATION_TEMPLATE_IDS = [
  "find_critical_bugs",
  "scan_codebase_vulnerabilities",
  "generate_docs",
  "add_test_coverage",
  "find_vulnerabilities",
  "assign_pr_reviewers",
  "autofix_pr_review_comments",
  "monitor_engineering_invariants",
  "remediate_dependency_vulnerabilities",
  "fix_bugs_reported_in_slack",
  "triage_failed_github_actions",
  "fix_ci_failures",
  "investigate_pagerduty_incidents",
  "investigate_sentry_issues",
  "investigate_top_datadog_errors",
  "triage_linear_issues",
  "summarize_changes_daily",
  "customer_health_monitoring",
  "product_analytics",
  "product_faq",
  "product_finance",
  "slack_digest",
  "investigate_environment_setup_failures",
  "monitor_environment_build_health",
] as const;

export type AutomationTemplateId = (typeof AUTOMATION_TEMPLATE_IDS)[number];

const WEEKDAYS: ScheduleConfig["days"] = { kind: "weekly", daysOfWeek: [1, 2, 3, 4, 5] };
const MONDAY: ScheduleConfig["days"] = { kind: "weekly", daysOfWeek: [1] };

const AT = (time: string): ScheduleConfig["time"] => ({ kind: "at", time });

export interface AutomationTemplateBase {
  id: AutomationTemplateId;
  prompt: string;
  trigger: TemplateTriggerId;
  action: TemplateActionId;
}

export type AutomationTemplate =
  | (AutomationTemplateBase & {
      triggerKind: "schedule";
      schedule: Pick<ScheduleConfig, "time" | "days">;
    })
  | (AutomationTemplateBase & {
      triggerKind: "webhook";
      eventFilters: WebhookEventFilter[];
    });

const SCHEDULED_SLACK = {
  trigger: "scheduled" as const,
  action: "send_slack" as const,
  triggerKind: "schedule" as const,
};

export const AUTOMATION_TEMPLATES: Record<AutomationTemplateId, AutomationTemplate> = {
  find_critical_bugs: {
    id: "find_critical_bugs",
    ...SCHEDULED_SLACK,
    schedule: { time: AT("09:00"), days: WEEKDAYS },
    prompt: `# Goal
Analyze recent commits for high-severity correctness bugs and submit only safe, well-scoped fixes.

# Context
This run is a scheduled correctness sweep, not a style pass. Prefer bugs that can break users, corrupt data, or violate an invariant over nits and speculative refactors.

# Steps
1. Collect commits and diffs from the last 24 hours on the default branch and on open pull requests.
2. Rank findings by user impact: crashes, wrong results, race conditions, broken authz, and silent data loss first.
3. For each high-severity candidate, reproduce from the diff and surrounding tests; discard anything you cannot explain with evidence.
4. Where a fix is local, low-risk, and covered by tests (or you can add a focused test), open a pull request with the diagnosis and the change.
5. Post a digest of confirmed bugs, skipped suspects, and any opened PRs, then notify the team.`,
  },
  scan_codebase_vulnerabilities: {
    id: "scan_codebase_vulnerabilities",
    ...SCHEDULED_SLACK,
    schedule: { time: AT("08:00"), days: MONDAY },
    prompt: `# Goal
Review the full repository on a schedule and alert only on validated, high-impact security issues.

# Context
Noise from scanners is worse than silence. Confirm exploitability or a realistic attack path before you file anything.

# Steps
1. Inventory the repo: auth boundaries, secret handling, deserialization, SSRF/injection sinks, and dependency surfaces.
2. Run or read the latest vulnerability scan output, then manually verify each high/critical hit against the current code.
3. Drop findings that are unreachable, already mitigated, or require an unrealistic attacker.
4. For each validated issue, write impact, reproduction, and a concrete remediation (patch, config, or dependency bump).
5. Post a ranked report and notify the team. Do not open drive-by refactors.`,
  },
  generate_docs: {
    id: "generate_docs",
    ...SCHEDULED_SLACK,
    schedule: { time: AT("14:00"), days: WEEKDAYS },
    prompt: `# Goal
Create and update developer documentation for recently changed or under-documented code.

# Context
Docs should match the code that shipped, not an idealized design. Prefer updating the existing doc tree over inventing a parallel one.

# Steps
1. List significant changes from the last 48 hours (APIs, config, CLI, data model, and user-visible behavior).
2. For each change, find the current doc page or README that should describe it; note gaps and stale sections.
3. Draft or update those pages with accurate examples, failure modes, and links to the owning code.
4. If a public API changed without a changelog entry, add one.
5. Open a documentation PR when the edits are substantial, and post a summary of what was documented vs still missing.`,
  },
  add_test_coverage: {
    id: "add_test_coverage",
    ...SCHEDULED_SLACK,
    schedule: { time: AT("10:00"), days: WEEKDAYS },
    prompt: `# Goal
Review recent changes and add tests for high-risk logic that lacks adequate coverage.

# Context
Do not chase coverage percentage. Target branches that can fail in production: auth, money, data writes, concurrency, and parsers.

# Steps
1. Diff the last 24–48 hours and list new or changed logic without nearby tests.
2. Rank gaps by blast radius if the code is wrong.
3. Add focused unit or integration tests that fail on the bug you are worried about; avoid snapshot-only tests.
4. Run the new tests and the nearest existing suite; do not merge a test file that does not run in CI.
5. Open a PR with the tests (and tiny production fixes only if a test proved a real bug), then notify the team.`,
  },
  find_vulnerabilities: {
    id: "find_vulnerabilities",
    trigger: "pr_opened",
    action: "pr_comment",
    triggerKind: "webhook",
    eventFilters: [{ event: "pull_request", actions: ["opened"] }],
    prompt: `# Goal
Review the triggering pull request for exploitable security issues and flag only validated findings before merge.

# Context
This run is gated on a PR-opened webhook. Comment on the PR itself. False positives block the team; stay quiet unless you can show a real issue.

# Steps
1. Read the PR description, diff, and related tests. Map trust boundaries the change touches (auth, input, secrets, deserialization, access checks).
2. Look for injection, SSRF, authz bypass, secret leakage, unsafe eval, and dependency changes that widen the attack surface.
3. Attempt to validate each suspicion against the diff. Drop theoretical or already-mitigated cases.
4. Comment with severity, the exact code path, and a recommended fix. Do not nitpick style.
5. If there are no validated findings, post a short all-clear rather than a wall of scanner output.`,
  },
  assign_pr_reviewers: {
    id: "assign_pr_reviewers",
    trigger: "pr_pushed",
    action: "request_reviewers",
    triggerKind: "webhook",
    eventFilters: [{ event: "pull_request", actions: ["synchronize"] }],
    prompt: `# Goal
Assign reviewers based on the files that changed, and auto-approve only genuinely low-risk PRs.

# Context
This run fires when a PR is pushed. Use CODEOWNERS, recent blame, and the diff — not a random rotation.

# Steps
1. Inspect the latest push: files touched, risk (auth, schema, infra vs docs/tests), and whether CI is green.
2. Choose reviewers who own the changed paths. Avoid piling on extra people for a docs-only change.
3. Request review from those people. If the PR is docs, comments, or lockfile-only with no runtime impact, auto-approve per repo policy.
4. Leave a short comment explaining why those reviewers (or why it was auto-approved).
5. If ownership is ambiguous, assign the most recent authors of the hottest files and say so.`,
  },
  autofix_pr_review_comments: {
    id: "autofix_pr_review_comments",
    trigger: "pr_review_comment",
    action: "pr_comment",
    triggerKind: "webhook",
    eventFilters: [{ event: "pull_request_review_comment", actions: ["created"] }],
    prompt: `# Goal
Take a first pass at addressing inline review comments on the PR diff.

# Context
This run fires on a new review comment. Fix what is clearly right; do not argue or rewrite the feature.

# Steps
1. Read the comment, the hunk it points at, and nearby tests.
2. If the request is a clear bug, missing test, or local cleanup, implement it on the PR branch.
3. If the request is a design debate, out of scope, or would change product behavior, reply with a concise question or a reason to skip.
4. Push the fix when you made one, and reply on the thread with what changed.
5. Never force-push over the author's unpushed work; never dismiss comments you did not address.`,
  },
  monitor_engineering_invariants: {
    id: "monitor_engineering_invariants",
    ...SCHEDULED_SLACK,
    schedule: { time: AT("08:30"), days: WEEKDAYS },
    prompt: `# Goal
Re-check critical repository invariants on a schedule and alert only when a rule regresses.

# Context
Invariants are the "this must stay true" rules in this repo (layering, no FK cascades, query-key shape, i18n parity, and similar). Alert on breakage, not on every run.

# Steps
1. Load the project's documented invariants (CLAUDE.md, CONTRIBUTING, lint rules, architecture tests).
2. Run the cheapest checks that prove those rules: targeted tests, ripgrep guards, and typecheck where relevant.
3. Compare with the last known-good result. Ignore pre-existing debt unless it got worse.
4. For a new regression, capture the failing evidence and a suggested fix location.
5. Notify the team only if something regressed; a clean run should stay quiet or one-line.`,
  },
  remediate_dependency_vulnerabilities: {
    id: "remediate_dependency_vulnerabilities",
    trigger: "issue_created",
    action: "send_slack",
    triggerKind: "webhook",
    eventFilters: [{ event: "issues", actions: ["opened"] }],
    prompt: `# Goal
Triage dependency-vulnerability tickets and open upgrade PRs when the fix is safe.

# Context
This run fires when a dependency advisory issue is created. Do not bump majors blindly.

# Steps
1. Read the ticket: package, CVE/advisory, severity, and the locked version in this repo.
2. Confirm the vulnerable code path is actually reachable from this project.
3. If a patch/minor upgrade exists and tests pass, open a PR that bumps only that dependency (and its required transitives).
4. If a major bump or API break is required, write a migration note and do not land it in this run.
5. Comment on the ticket with the decision, and notify the team of any opened PR or accepted risk.`,
  },
  fix_bugs_reported_in_slack: {
    id: "fix_bugs_reported_in_slack",
    trigger: "new_message_in_channel",
    action: "send_slack",
    triggerKind: "webhook",
    eventFilters: [{ event: "message" }],
    prompt: `# Goal
Monitor a channel for bug reports, investigate the codebase, and fix with a pull request when the issue is real and scoped.

# Context
This run fires on a new message in the configured channel. Ignore chatter, emoji, and already-threaded reports.

# Steps
1. Parse the message for a reproducible symptom, environment, and any links or stack traces.
2. Search the codebase and recent issues for the same failure. Duplicate? Reply with the existing thread and stop.
3. If it is new, reproduce or reason from logs/code. File or update a tracked issue with severity and next steps.
4. For a small, high-confidence fix, open a PR and reply in the channel with the diagnosis and PR link.
5. If you cannot reproduce or the change is large, reply with what you know and what a human should do next.`,
  },
  triage_failed_github_actions: {
    id: "triage_failed_github_actions",
    trigger: "workflow_run_completed",
    action: "send_slack",
    triggerKind: "webhook",
    eventFilters: [{ event: "workflow_run", actions: ["completed"] }],
    prompt: `# Goal
Investigate failed or cancelled workflow runs and report findings to the team.

# Context
This run fires when a workflow run completes. Skip green runs. Flakes and real breaks need different treatment.

# Steps
1. Read the workflow name, branch, event, and failed job logs.
2. Classify: flake (timeout, lost runner, known-flaky test), product regression, infra/misconfig, or cancelled by a newer run.
3. For a regression, identify the first failing test or step and the commit that likely introduced it.
4. For a flake, note the pattern and whether a retry already passed.
5. Post a short report: cause, evidence, and recommended action (retry, revert, or a follow-up PR).`,
  },
  fix_ci_failures: {
    id: "fix_ci_failures",
    trigger: "checks_completed",
    action: "send_slack",
    triggerKind: "webhook",
    eventFilters: [{ event: "check_suite", actions: ["completed"] }],
    prompt: `# Goal
Detect CI failures on the default branch and automatically open PRs for failures you can fix safely.

# Context
This run fires when checks complete. Only act on the default branch. Do not fight in-progress feature branches.

# Steps
1. If checks passed or the ref is not the default branch, stop.
2. Read the failing jobs. Separate infra flakes from deterministic test/type/lint failures.
3. For a deterministic failure, reproduce locally in reasoning from the log, then apply the smallest fix.
4. Open a PR with the failing command in the description and the log excerpt.
5. Notify the team with the PR (or with "flake, retried / needs infra" if you did not change code).`,
  },
  investigate_pagerduty_incidents: {
    id: "investigate_pagerduty_incidents",
    trigger: "incident_triggered",
    action: "datadog",
    triggerKind: "webhook",
    eventFilters: [{ event: "incident" }],
    prompt: `# Goal
Investigate a newly triggered incident using observability data and code context, then write a first-pass diagnosis.

# Context
This run fires when an incident is triggered. Speed and a clear timeline matter more than a perfect root cause.

# Steps
1. Read the incident title, service, severity, and recent related alerts.
2. Pull the overlapping metrics, traces, and logs around the trigger time. Note deploys that landed in the same window.
3. Map symptoms onto code paths (handlers, queues, cron, feature flags).
4. Write a timeline: detection → impact → likely cause → blast radius → immediate mitigations (rollback, feature flag, scale).
5. Post the diagnosis back on the incident and to the team. Do not declare resolved unless evidence is conclusive.`,
  },
  investigate_sentry_issues: {
    id: "investigate_sentry_issues",
    trigger: "sentry_issue_event",
    action: "send_slack",
    triggerKind: "webhook",
    eventFilters: [{ event: "error" }],
    prompt: `# Goal
Investigate errors from Sentry, identify root causes, and propose fixes.

# Context
This run fires on a Sentry issue event. New issues and sudden volume spikes matter; one-off noise does not.

# Steps
1. Read the event: exception, stack, release, environment, user/volume trend.
2. Locate the failing code and the last change that touched it.
3. Decide: new regression, known issue, or telemetry noise.
4. If it is a regression with a small fix, open a PR; otherwise write a repro and a recommended patch.
5. Notify the team with the Sentry link, impact, and next step.`,
  },
  investigate_top_datadog_errors: {
    id: "investigate_top_datadog_errors",
    trigger: "scheduled",
    action: "datadog",
    triggerKind: "schedule",
    schedule: { time: AT("09:30"), days: WEEKDAYS },
    prompt: `# Goal
Investigate recurring production errors from Datadog, identify root causes, and propose fixes.

# Context
This is a scheduled look at the error budget, not a page. Focus on the top recurring faults, not yesterday's one-offs.

# Steps
1. Pull the top error signatures by count and user impact over the last 24 hours.
2. For each new or worsening signature, inspect traces/logs and the owning code.
3. Group duplicates. Skip errors already covered by an open issue/PR unless volume jumped.
4. Propose or land a fix when it is safe; otherwise file a tracked issue with evidence.
5. Write a short ranked digest back to Datadog/the team.`,
  },
  triage_linear_issues: {
    id: "triage_linear_issues",
    trigger: "issue_created",
    action: "send_slack",
    triggerKind: "webhook",
    eventFilters: [{ event: "issues", actions: ["opened"] }],
    prompt: `# Goal
Triage new issues by investigating bugs, planning feature requests, and opening PRs for easy fixes.

# Context
This run fires when an issue is created. Label, prioritize, and only code when the change is obviously small.

# Steps
1. Read the new issue: type (bug vs feature), repro, and any screenshots/logs.
2. Search the codebase and existing issues for duplicates; link and close as duplicate when sure.
3. For bugs: reproduce, set severity, and if the fix is local, open a PR.
4. For features: outline scope, unknowns, and a suggested breakdown — do not start a large implementation in this run.
5. Comment on the issue with the triage result and notify the team.`,
  },
  summarize_changes_daily: {
    id: "summarize_changes_daily",
    ...SCHEDULED_SLACK,
    schedule: { time: AT("09:00"), days: WEEKDAYS },
    prompt: `# Goal
Post a daily digest summarizing notable repository changes and risks from the previous day.

# Context
This is a briefing for humans who did not read every PR. Call out risk, not every changelog line.

# Steps
1. Collect merged PRs, notable commits, incidents, and CI redness from the previous calendar day.
2. Group by theme: shipped features, fixes, infra, and anything that looks risky (migrations, auth, public API).
3. For each item, write one or two sentences: what changed, why it matters, and follow-up if needed.
4. Flag open questions (reverts, failing default-branch CI, unresolved incidents).
5. Post the digest and notify the team.`,
  },
  customer_health_monitoring: {
    id: "customer_health_monitoring",
    trigger: "scheduled",
    action: "granola",
    triggerKind: "schedule",
    schedule: { time: AT("09:00"), days: WEEKDAYS },
    prompt: `# Goal
Find at-risk customers using usage analytics, call notes, Slack escalations, and issue blockers.

# Context
"At risk" means declining usage, unpaid/expand friction, unresolved severity, or a string of support threads — not a single quiet day.

# Steps
1. Pull usage and activation signals for the last 7 and 30 days; mark accounts with a sharp drop or failed expansion.
2. Read recent call notes and meeting recaps for churn language, competitors, and stalled rollouts.
3. Cross-check Slack escalations and open blockers on the account.
4. Rank accounts by urgency with evidence and a recommended owner action.
5. Write the briefing into the notes dest and notify the team for anything that needs a same-day response.`,
  },
  product_analytics: {
    id: "product_analytics",
    trigger: "scheduled",
    action: "databricks_sql",
    triggerKind: "schedule",
    schedule: { time: AT("10:00"), days: MONDAY },
    prompt: `# Goal
Produce a weekly product usage, activation, retention, and feature-adoption digest from warehouse data.

# Context
Numbers without a comparison are trivia. Always show WoW/WoW and call out the segments that moved.

# Steps
1. Query activation, retention, and feature adoption for the last week vs the prior week.
2. Break out the movements that matter (new vs existing, plan, platform) — skip vanity totals.
3. Investigate any cliff: a chart that dropped should get a hypothesis (release, outage, seasonality).
4. List 3–5 insights a PM can act on, each with the query/metric behind it.
5. Publish the digest and keep the SQL auditable.`,
  },
  product_faq: {
    id: "product_faq",
    trigger: "new_message_in_channel",
    action: "notion",
    triggerKind: "webhook",
    eventFilters: [{ event: "message" }],
    prompt: `# Goal
Answer product questions in a dedicated channel using Slack, Notion, issue, and repository context.

# Context
This run fires on a new message in the FAQ channel. Prefer citing the live doc over inventing policy.

# Steps
1. Read the question. If it is not a product question, ignore or point the person to the right place.
2. Search Notion, recent issues, and the repo/docs for the current answer.
3. Reply in the channel with a short answer and links. Flag uncertainty instead of guessing.
4. If the docs are wrong or missing, draft a Notion update and say you did.
5. Never leak private customer data into a public channel.`,
  },
  product_finance: {
    id: "product_finance",
    trigger: "scheduled",
    action: "stripe",
    triggerKind: "schedule",
    schedule: { time: AT("09:00"), days: MONDAY },
    prompt: `# Goal
Analyze Stripe revenue, churn signals, and product pricing opportunities.

# Context
This is a weekly finance pass. Treat payment data as sensitive; aggregate unless a specific account is already in an incident.

# Steps
1. Pull MRR/revenue, new, expansion, contraction, and churn for the last week vs the prior week.
2. List failed payments, delinquent invoices, and repeated dunning as operational risk.
3. Note pricing or packaging issues that show up in tickets or lost expansions.
4. Propose at most three concrete follow-ups (retry, outreach, packaging experiment) with evidence.
5. Publish the digest to the finance dest. Do not export raw cardholder data.`,
  },
  slack_digest: {
    id: "slack_digest",
    trigger: "scheduled",
    action: "slack",
    triggerKind: "schedule",
    schedule: { time: AT("18:00"), days: WEEKDAYS },
    prompt: `# Goal
Summarize important DMs, mentions, and the user's top active Slack channels.

# Context
This is an end-of-day catch-up. People, decisions, and asks — not a transcript dump.

# Steps
1. Collect unread DMs, @mentions, and high-activity channels from the last workday.
2. Cluster by thread. Drop automated noise (CI bots, deploy spam) unless it is an incident.
3. For each cluster, write: who, what they need, and whether a reply is still owed.
4. Highlight anything time-sensitive for tomorrow morning.
5. Post the digest privately to the user. Do not forward private DMs into a public channel.`,
  },
  investigate_environment_setup_failures: {
    id: "investigate_environment_setup_failures",
    ...SCHEDULED_SLACK,
    schedule: { time: AT("08:00"), days: WEEKDAYS },
    prompt: `# Goal
Analyze recent cloud agent runs to root-cause environment setup failures from setup logs, transcripts, and related issues.

# Context
Setup failures waste every subsequent step. Look for missing tools, auth, network, and image drift — not application bugs.

# Steps
1. List recent runs that failed during environment setup (install, image pull, secrets, network, toolchain).
2. Read setup logs and transcripts. Group identical failures.
3. Identify the first error that actually caused the rest of the log.
4. Propose a durable fix (image pin, secret, network allowlist, docs) and a short-term workaround.
5. Post a ranked report of causes and recommended owners.`,
  },
  monitor_environment_build_health: {
    id: "monitor_environment_build_health",
    ...SCHEDULED_SLACK,
    schedule: { time: AT("08:15"), days: WEEKDAYS },
    prompt: `# Goal
Health-check cloud environment builds and perform root-cause analysis for failed builds.

# Context
This is a scheduled build-health pass. A single red build is a data point; a rising fail rate is the story.

# Steps
1. Collect environment build outcomes for the last 24 hours: success rate, duration, and fail classification.
2. Compare with the previous week. Call out new failure modes and duration regressions.
3. For failed builds, read logs enough to name the cause (compile, test, image, quota, flake).
4. File or update an issue when a cause repeats; open a PR only for a small, proven fix.
5. Post the health summary and any actions taken.`,
  },
};

export interface TemplateCategory {
  id: TemplateCategoryId;
  templateIds: readonly AutomationTemplateId[];
}

export const TEMPLATE_CATEGORIES: readonly TemplateCategory[] = [
  {
    id: "popular",
    templateIds: [
      "find_critical_bugs",
      "scan_codebase_vulnerabilities",
      "generate_docs",
      "add_test_coverage",
    ],
  },
  {
    id: "code_review",
    templateIds: [
      "find_vulnerabilities",
      "assign_pr_reviewers",
      "add_test_coverage",
      "autofix_pr_review_comments",
      "find_critical_bugs",
      "monitor_engineering_invariants",
    ],
  },
  {
    id: "security",
    templateIds: [
      "find_vulnerabilities",
      "remediate_dependency_vulnerabilities",
      "scan_codebase_vulnerabilities",
    ],
  },
  {
    id: "incidents_triage",
    templateIds: [
      "fix_bugs_reported_in_slack",
      "triage_failed_github_actions",
      "fix_ci_failures",
      "investigate_pagerduty_incidents",
      "investigate_sentry_issues",
      "investigate_top_datadog_errors",
      "triage_linear_issues",
    ],
  },
  {
    id: "data_research",
    templateIds: [
      "summarize_changes_daily",
      "customer_health_monitoring",
      "product_analytics",
      "product_faq",
      "product_finance",
      "slack_digest",
    ],
  },
  {
    id: "environment",
    templateIds: [
      "investigate_environment_setup_failures",
      "monitor_environment_build_health",
    ],
  },
];
