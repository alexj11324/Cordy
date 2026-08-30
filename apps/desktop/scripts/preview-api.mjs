import enCommon from "../../../packages/views/locales/en/common.json" with { type: "json" };
import jaCommon from "../../../packages/views/locales/ja/common.json" with { type: "json" };
import koCommon from "../../../packages/views/locales/ko/common.json" with { type: "json" };
import zhHansCommon from "../../../packages/views/locales/zh-Hans/common.json" with { type: "json" };

const WORKSPACE_ID = "ws-preview";
const PREVIEW_USER_ID = "user-preview";
const PREVIEW_MEMBER_ID = "member-preview";
const PREVIEW_AGENT_ID = "agent-preview";
const PREVIEW_TIMEZONE = "America/New_York";

const STATUS_CATEGORIES = [
  "backlog",
  "todo",
  "in_progress",
  "in_review",
  "done",
  "blocked",
  "cancelled",
];

// Keep fixture timestamps anchored to the process that serves the preview.
// Hard-coded dates make the shared time-ago cells look stale as soon as the
// demo is revisited on another day.
const PREVIEW_SESSION_STARTED_AT = Date.now();
const NOW = new Date(PREVIEW_SESSION_STARTED_AT).toISOString();

const PREVIEW_COPY_BY_LOCALE = {
  en: enCommon.preview.fixtures,
  ja: jaCommon.preview.fixtures,
  ko: koCommon.preview.fixtures,
  "zh-Hans": zhHansCommon.preview.fixtures,
};
const DEFAULT_PREVIEW_COPY = PREVIEW_COPY_BY_LOCALE.en;
const PREVIEW_LOCALES = [
  { tag: "en", copy: PREVIEW_COPY_BY_LOCALE.en },
  { tag: "ja", copy: PREVIEW_COPY_BY_LOCALE.ja },
  { tag: "ko", copy: PREVIEW_COPY_BY_LOCALE.ko },
  { tag: "zh", copy: PREVIEW_COPY_BY_LOCALE["zh-Hans"] },
];

function previewCopyForRequest(req) {
  const header = String(req.headers?.["accept-language"] ?? "");
  const preferences = header
    .split(",")
    .map((part, index) => {
      const [rawTag, ...parameters] = part.trim().split(";");
      const tag = rawTag?.trim().toLowerCase();
      if (!tag || tag === "*") return null;
      const qualityParameter = parameters.find((parameter) =>
        parameter.trim().toLowerCase().startsWith("q="),
      );
      const quality = qualityParameter
        ? Number(qualityParameter.trim().slice(2))
        : 1;
      return Number.isFinite(quality) && quality > 0 && quality <= 1
        ? { tag, quality, index }
        : null;
    })
    .filter((preference) => preference !== null)
    .sort((left, right) => right.quality - left.quality || left.index - right.index);

  for (const preference of preferences) {
    const locale = PREVIEW_LOCALES.find(
      ({ tag }) =>
        preference.tag === tag ||
        preference.tag.startsWith(`${tag}-`) ||
        tag.startsWith(`${preference.tag}-`),
    );
    if (locale) return locale.copy;
  }
  return DEFAULT_PREVIEW_COPY;
}

function previewTime(minutesFromSessionStart) {
  return new Date(
    PREVIEW_SESSION_STARTED_AT + minutesFromSessionStart * 60_000,
  ).toISOString();
}

function previewZonedParts(timestamp, timeZone) {
  const parts = Object.fromEntries(
    new Intl.DateTimeFormat("en-US", {
      timeZone,
      calendar: "gregory",
      hourCycle: "h23",
      year: "numeric",
      month: "numeric",
      day: "numeric",
      hour: "numeric",
      minute: "numeric",
      weekday: "short",
    })
      .formatToParts(new Date(timestamp))
      .filter(({ type }) => type !== "literal")
      .map(({ type, value }) => [type, value]),
  );
  return {
    year: Number(parts.year),
    month: Number(parts.month),
    day: Number(parts.day),
    hour: Number(parts.hour),
    minute: Number(parts.minute),
    weekday: ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"].indexOf(parts.weekday),
  };
}

function previewCronFieldMatches(value, field, minimum, maximum) {
  return field.split(",").some((token) => {
    const [range, rawStep] = token.split("/");
    const step = rawStep === undefined ? 1 : Number(rawStep);
    if (!Number.isInteger(step) || step < 1) return false;

    let start = minimum;
    let end = maximum;
    if (range !== "*") {
      if (range.includes("-")) {
        const [rawStart, rawEnd] = range.split("-");
        start = Number(rawStart);
        end = Number(rawEnd);
      } else {
        start = Number(range);
        end = start;
      }
    }
    return Number.isInteger(start) && Number.isInteger(end) &&
      start >= minimum && end <= maximum && start <= end &&
      value >= start && value <= end && (value - start) % step === 0;
  });
}

function previewNextCronOccurrence(cronExpression, timeZone, after = PREVIEW_SESSION_STARTED_AT) {
  const fields = cronExpression.trim().split(/\s+/);
  if (fields.length !== 5) throw new Error(`Unsupported preview cron: ${cronExpression}`);
  const [minuteField, hourField, dayOfMonthField, monthField, dayOfWeekField] = fields;
  const firstMinute = Math.floor(after / 60_000) + 1;
  const maxMinutes = 366 * 24 * 60;

  for (let offset = 0; offset <= maxMinutes; offset += 1) {
    const timestamp = (firstMinute + offset) * 60_000;
    const parts = previewZonedParts(timestamp, timeZone);
    if (!previewCronFieldMatches(parts.minute, minuteField, 0, 59)) continue;
    if (!previewCronFieldMatches(parts.hour, hourField, 0, 23)) continue;
    if (!previewCronFieldMatches(parts.month, monthField, 1, 12)) continue;
    const dayOfMonthMatches = previewCronFieldMatches(parts.day, dayOfMonthField, 1, 31);
    const dayOfWeekMatches = previewCronFieldMatches(parts.weekday, dayOfWeekField, 0, 6);
    const dayOfMonthWildcard = dayOfMonthField === "*";
    const dayOfWeekWildcard = dayOfWeekField === "*";
    const dayMatches = dayOfMonthWildcard || dayOfWeekWildcard
      ? dayOfMonthMatches && dayOfWeekMatches
      : dayOfMonthMatches || dayOfWeekMatches;
    if (dayMatches) return new Date(timestamp).toISOString();
  }

  throw new Error(`No preview cron occurrence found: ${cronExpression}`);
}

const PREVIEW_NEXT_PR_REVIEW_AT = previewNextCronOccurrence(
  "*/30 * * * *",
  PREVIEW_TIMEZONE,
);
const PREVIEW_NEXT_CI_WATCH_AT = previewNextCronOccurrence(
  "*/15 * * * *",
  PREVIEW_TIMEZONE,
);

const PREVIEW_WORKSPACE = {
  id: WORKSPACE_ID,
  name: "Preview",
  slug: "preview",
  description: null,
  context: null,
  settings: {},
  repos: [],
  issue_prefix: "PRE",
  avatar_url: null,
  created_at: NOW,
  updated_at: NOW,
};

const PREVIEW_MEMBER = {
  id: PREVIEW_MEMBER_ID,
  workspace_id: WORKSPACE_ID,
  user_id: PREVIEW_USER_ID,
  role: "owner",
  created_at: NOW,
  name: "Alex",
  email: "preview@local",
  avatar_url: null,
};

const PREVIEW_ISSUES = [
  previewIssue("101", "backlog", "Refine workspace onboarding", "Make the first-run path easier to understand.", "high"),
  previewIssue("102", "todo", "Polish issue board empty states", "Keep the board useful before real work arrives.", "medium", "member", null, previewTime(-29)),
  previewIssue("103", "todo", "Add keyboard shortcuts", "Expose the common actions without extra chrome.", "low", "member", null, previewTime(-20)),
  previewIssue("104", "in_progress", "Add real-time status indicator", "Show when an agent is actively working on an issue.", "urgent", "agent", null, previewTime(-242)),
  previewIssue(
    "105",
    "in_review",
    "Check responsive sidebar",
    "Make the workspace navigation feel balanced at every width.",
    "medium",
    "agent",
    { type: "agent", id: "agent-mika" },
    previewTime(-1501),
  ),
  previewIssue("106", "done", "Split web and API dev commands", "Let visual work start without the full local stack.", "none", "member", null, previewTime(-4322)),
];

const PREVIEW_DIRECTORY_AGENT = {
  id: PREVIEW_AGENT_ID,
  workspace_id: WORKSPACE_ID,
  runtime_id: "runtime-preview",
  runtime_bound: false,
  name: "Coding",
  description: "Preview agent",
  instructions: "",
  avatar_url: null,
  runtime_mode: "local",
  runtime_config: {},
  custom_args: [],
  visibility: "workspace",
  permission_mode: "public_to",
  invocation_targets: [{ target_type: "workspace", target_id: null }],
  status: "working",
  max_concurrent_tasks: 1,
  model: "preview",
  owner_id: PREVIEW_USER_ID,
  skills: [],
  created_at: NOW,
  updated_at: NOW,
  archived_at: null,
  archived_by: null,
};

const PREVIEW_MEMBERS = [
  PREVIEW_MEMBER,
  {
    id: "member-sam",
    workspace_id: WORKSPACE_ID,
    user_id: "user-sam",
    role: "member",
    created_at: NOW,
    name: "Sam Rivera",
    email: "sam@preview.local",
    avatar_url: null,
  },
];

// Optional panels mounted by the shared Agent and Runtime pages still issue
// their normal read queries in the browser preview. Resolve those reads with
// explicit empty contracts so opening a tab does not turn an intentionally
// disconnected demo into a wall of failed requests.
const PREVIEW_EMPTY_INSTALLATIONS = {
  installations: [],
  configured: false,
  install_supported: false,
};
const PREVIEW_EMPTY_DINGTALK_GROUP_ROUTES = { routes: [] };
const PREVIEW_EMPTY_RUNTIME_PROFILES = { runtime_profiles: [] };
const PREVIEW_EMPTY_PLUGINS = { plugins: [] };
const PREVIEW_EMPTY_MCP_SERVERS = [];
const PREVIEW_CHAT_SESSION_ID = "chat-session-preview-mika";
const PREVIEW_EMPTY_CHAT_PENDING_TASK = {
  supports_queue: false,
  queued_tasks: [],
};

function previewRuntimeLocalSkills(runtimeId) {
  return {
    id: `preview-local-skills-${runtimeId}`,
    runtime_id: runtimeId,
    status: "completed",
    skills: [],
    supported: true,
    mcp_servers: [],
    mcp_supported: false,
    created_at: NOW,
    updated_at: NOW,
  };
}

function previewRuntimeModels(runtimeId) {
  return {
    id: `preview-models-${runtimeId}`,
    runtime_id: runtimeId,
    status: "completed",
    models: [],
    supported: true,
    created_at: NOW,
    updated_at: NOW,
  };
}

function previewChatMessagesPage(url) {
  const requestedLimit = Number(url.searchParams.get("limit") ?? 50);
  const limit = Number.isInteger(requestedLimit) && requestedLimit > 0
    ? requestedLimit
    : 50;
  return {
    messages: [],
    limit,
    has_more: false,
    next_cursor: null,
  };
}

function previewRuntime(id, name, provider, status) {
  return {
    id,
    workspace_id: WORKSPACE_ID,
    daemon_id: status === "online" ? `daemon-${id}` : null,
    name,
    custom_name: null,
    runtime_mode: "local",
    provider,
    launch_header: "",
    status,
    device_info: status === "online" ? "Browser preview runtime" : "Runtime offline",
    metadata: { preview: true },
    owner_id: PREVIEW_USER_ID,
    visibility: "private",
    profile_id: null,
    last_seen_at: status === "online" ? NOW : previewTime(-60),
    created_at: NOW,
    updated_at: NOW,
  };
}

const PREVIEW_RUNTIMES = [
  previewRuntime("runtime-preview", "Preview runtime · Atlas", "codex", "online"),
  previewRuntime("runtime-mika", "Preview runtime · Mika", "claude", "online"),
  previewRuntime("runtime-nova", "Preview runtime · Nova", "codex", "online"),
  previewRuntime("runtime-quill", "Preview runtime · Quill", "codex", "offline"),
];

function previewAgent(id, name, runtimeId, status, description, systemKey = null) {
  return {
    id,
    workspace_id: WORKSPACE_ID,
    runtime_id: runtimeId,
    runtime_bound: true,
    name,
    description,
    instructions: `You are ${name}, a sample agent in the local preview.`,
    avatar_url: null,
    runtime_mode: "local",
    runtime_config: {},
    custom_args: [],
    visibility: "workspace",
    permission_mode: "public_to",
    invocation_targets: [{ target_type: "workspace", target_id: null }],
    status,
    max_concurrent_tasks: 2,
    model: "preview",
    thinking_level: "high",
    service_tier: "",
    owner_id: PREVIEW_USER_ID,
    skills: [],
    created_at: NOW,
    updated_at: NOW,
    archived_at: null,
    archived_by: null,
    ...(systemKey ? { system_key: systemKey } : {}),
  };
}

const PREVIEW_DIRECTORY_AGENTS = [
  {
    ...PREVIEW_DIRECTORY_AGENT,
    name: "Atlas",
    description: "Builds features and prepares implementation handoffs.",
    instructions: "You are Atlas, a sample agent in the local preview.",
    runtime_bound: true,
    runtime_id: "runtime-preview",
    thinking_level: "high",
    service_tier: "",
  },
  previewAgent(
    "agent-mika",
    "Mika",
    "runtime-mika",
    "working",
    "Reviews completed work and sends actionable feedback.",
    "mika",
  ),
  previewAgent(
    "agent-nova",
    "Nova",
    "runtime-nova",
    "idle",
    "Runs CI checks and closes validated tasks.",
  ),
  {
    ...previewAgent(
      "agent-quill",
      "Quill",
      "runtime-quill",
      "offline",
      "Prepares scheduled workspace summaries.",
    ),
    runtime_bound: true,
  },
];

function previewTask({
  id,
  agentId,
  runtimeId,
  issueNumber,
  status,
  createdAt,
  dispatchedAt = null,
  startedAt = null,
  completedAt = null,
  result = null,
  kind = "direct",
  autopilotRunId,
  triggerSummary,
  handoffNote,
  branchName,
}) {
  return {
    id,
    agent_id: agentId,
    runtime_id: runtimeId,
    issue_id: issueNumber ? issueId(issueNumber) : "",
    status,
    priority: 3,
    dispatched_at: dispatchedAt,
    started_at: startedAt,
    completed_at: completedAt,
    result,
    error: null,
    created_at: createdAt,
    kind,
    ...(autopilotRunId ? { autopilot_run_id: autopilotRunId } : {}),
    ...(triggerSummary ? { trigger_summary: triggerSummary } : {}),
    ...(handoffNote ? { handoff_note: handoffNote } : {}),
    ...(branchName ? { branch_name: branchName } : {}),
  };
}

const PREVIEW_TASKS = [
  previewTask({
    id: "task-pre-102",
    agentId: "agent-mika",
    runtimeId: "runtime-mika",
    issueNumber: "102",
    status: "queued",
    createdAt: previewTime(-28),
    autopilotRunId: "run-pr-review-queued",
    kind: "autopilot",
    triggerSummary: "PR review handoff",
    handoffNote: "Pick up the next unreviewed board state.",
  }),
  previewTask({
    id: "task-pre-103",
    agentId: "agent-nova",
    runtimeId: "runtime-nova",
    issueNumber: "103",
    status: "dispatched",
    createdAt: previewTime(-19),
    dispatchedAt: previewTime(-18),
    handoffNote: "Verify the keyboard path in the browser preview.",
  }),
  previewTask({
    id: "task-pre-104",
    agentId: PREVIEW_AGENT_ID,
    runtimeId: "runtime-preview",
    issueNumber: "104",
    status: "running",
    createdAt: previewTime(-240),
    dispatchedAt: previewTime(-239),
    startedAt: previewTime(-238),
    autopilotRunId: "run-ci-watch",
    kind: "autopilot",
    triggerSummary: "CI watch",
    handoffNote: "Keep watching CI and report the first actionable failure.",
  }),
  previewTask({
    id: "task-pre-105-implementation",
    agentId: PREVIEW_AGENT_ID,
    runtimeId: "runtime-preview",
    issueNumber: "105",
    status: "completed",
    createdAt: previewTime(-1500),
    dispatchedAt: previewTime(-1499),
    startedAt: previewTime(-1498),
    completedAt: previewTime(-120),
    result: { summary: "Responsive sidebar implementation is ready for review." },
    branchName: "feature/responsive-sidebar",
    handoffNote: "Implementation complete; route the task to a reviewer.",
  }),
  previewTask({
    id: "task-pre-105-review",
    agentId: "agent-mika",
    runtimeId: "runtime-mika",
    issueNumber: "105",
    status: "running",
    createdAt: previewTime(-99),
    dispatchedAt: previewTime(-98),
    startedAt: previewTime(-97),
    autopilotRunId: "run-pr-review-current",
    kind: "autopilot",
    triggerSummary: "PR review handoff",
    handoffNote: "Review the implementation handoff and leave findings on the PR.",
  }),
  previewTask({
    id: "task-pre-106",
    agentId: "agent-mika",
    runtimeId: "runtime-mika",
    issueNumber: "106",
    status: "completed",
    createdAt: previewTime(-4320),
    dispatchedAt: previewTime(-4319),
    startedAt: previewTime(-4318),
    completedAt: previewTime(-4290),
    result: { summary: "The web and API commands are split and documented." },
    autopilotRunId: "run-pr-review-completed",
    kind: "autopilot",
    triggerSummary: "PR review handoff",
    branchName: "chore/split-dev-commands",
  }),
];

const PREVIEW_ACTIVITY = [
  { agent_id: PREVIEW_AGENT_ID, bucket_at: previewTime(-240), task_count: 1, failed_count: 0 },
  { agent_id: "agent-mika", bucket_at: previewTime(-1440), task_count: 1, failed_count: 0 },
  { agent_id: "agent-nova", bucket_at: previewTime(-4320), task_count: 1, failed_count: 0 },
];

const PREVIEW_RUN_COUNTS = [
  { agent_id: PREVIEW_AGENT_ID, run_count: 4 },
  { agent_id: "agent-mika", run_count: 7 },
  { agent_id: "agent-nova", run_count: 12 },
  { agent_id: "agent-quill", run_count: 2 },
];

function previewAutopilot({
  id,
  title,
  description,
  assigneeId,
  status,
  executionMode,
  issueTitleTemplate = null,
  nextRunAt,
  triggerKinds,
  createdAt,
  pauseReason = null,
}) {
  return {
    id,
    workspace_id: WORKSPACE_ID,
    title,
    description,
    project_id: null,
    assignee_type: "agent",
    assignee_id: assigneeId,
    status,
    pause_reason: pauseReason,
    execution_mode: executionMode,
    issue_title_template: executionMode === "create_issue" ? issueTitleTemplate : null,
    created_by_type: "member",
    created_by_id: PREVIEW_USER_ID,
    last_run_at: null,
    created_at: createdAt,
    updated_at: NOW,
    trigger_kinds: triggerKinds,
    next_run_at: nextRunAt,
    last_run_status: null,
    subscribers: [{ user_type: "member", user_id: PREVIEW_USER_ID, created_at: NOW }],
    // This is a read-only local fixture. Mutations deliberately fall through
    // to Vite instead of pretending that preview writes reached a backend.
    can_write: false,
    can_manage_access: false,
  };
}

const PREVIEW_AUTOPILOTS = [
  previewAutopilot({
    id: "autopilot-pr-review",
    title: "PR review handoff",
    description: "Routes completed implementation work to an available reviewer.",
    assigneeId: "agent-mika",
    status: "active",
    executionMode: "create_issue",
    issueTitleTemplate: "PR review follow-up",
    nextRunAt: PREVIEW_NEXT_PR_REVIEW_AT,
    triggerKinds: ["schedule"],
    createdAt: previewTime(-4322),
  }),
  previewAutopilot({
    id: "autopilot-ci-watch",
    title: "CI watch",
    description: "Keeps an eye on checks for the active implementation branch.",
    assigneeId: PREVIEW_AGENT_ID,
    status: "active",
    executionMode: "create_issue",
    issueTitleTemplate: "CI watch follow-up",
    nextRunAt: PREVIEW_NEXT_CI_WATCH_AT,
    triggerKinds: ["schedule"],
    createdAt: previewTime(-270),
  }),
  previewAutopilot({
    id: "autopilot-weekly-summary",
    title: "Weekly workspace summary",
    description: "Creates a short digest of recently completed work.",
    assigneeId: "agent-quill",
    status: "paused",
    executionMode: "create_issue",
    issueTitleTemplate: "Weekly automation summary",
    nextRunAt: null,
    triggerKinds: ["schedule"],
    createdAt: previewTime(-10110),
    pauseReason: "agent_runtime_required",
  }),
];

function previewTrigger(
  id,
  autopilotId,
  enabled,
  cronExpression,
  nextRunAt,
  lastFiredAt = enabled ? previewTime(-30) : null,
) {
  return {
    id,
    autopilot_id: autopilotId,
    kind: "schedule",
    enabled,
    cron_expression: cronExpression,
    timezone: PREVIEW_TIMEZONE,
    next_run_at: nextRunAt,
    webhook_token: null,
    webhook_path: null,
    webhook_url: null,
    label: "Preview schedule",
    event_filters: null,
    last_fired_at: lastFiredAt,
    created_at: NOW,
    updated_at: NOW,
  };
}

const PREVIEW_TRIGGERS = {
  "autopilot-pr-review": [previewTrigger("trigger-pr-review", "autopilot-pr-review", true, "*/30 * * * *", PREVIEW_NEXT_PR_REVIEW_AT, previewTime(-97))],
  "autopilot-ci-watch": [previewTrigger("trigger-ci-watch", "autopilot-ci-watch", true, "*/15 * * * *", PREVIEW_NEXT_CI_WATCH_AT, previewTime(-241))],
  "autopilot-weekly-summary": [previewTrigger("trigger-weekly-summary", "autopilot-weekly-summary", false, "0 9 * * 1", null)],
};

function previewRun({
  id,
  autopilotId,
  triggerId,
  source,
  status,
  issueNumber = null,
  taskId = null,
  triggeredAt,
  completedAt = null,
  failureReason = null,
  reasonCode,
  result = null,
}) {
  return {
    id,
    autopilot_id: autopilotId,
    trigger_id: triggerId,
    source,
    status,
    issue_id: issueNumber ? issueId(issueNumber) : null,
    task_id: taskId,
    triggered_at: triggeredAt,
    completed_at: completedAt,
    failure_reason: failureReason,
    ...(reasonCode ? { reason_code: reasonCode } : {}),
    trigger_payload: { preview: true },
    result,
    created_at: triggeredAt,
  };
}

const PREVIEW_RUNS = {
  "autopilot-pr-review": [
    previewRun({
      id: "run-pr-review-current",
      autopilotId: "autopilot-pr-review",
      triggerId: "trigger-pr-review",
      source: "schedule",
      status: "running",
      issueNumber: "105",
      taskId: "task-pre-105-review",
      triggeredAt: previewTime(-97),
    }),
    previewRun({
      id: "run-pr-review-completed",
      autopilotId: "autopilot-pr-review",
      triggerId: "trigger-pr-review",
      source: "schedule",
      status: "completed",
      issueNumber: "106",
      taskId: "task-pre-106",
      triggeredAt: previewTime(-4321),
      completedAt: previewTime(-4262),
      result: { summary: "Review passed and the issue was closed." },
    }),
    previewRun({
      id: "run-pr-review-queued",
      autopilotId: "autopilot-pr-review",
      triggerId: "trigger-pr-review",
      source: "schedule",
      status: "issue_created",
      issueNumber: "102",
      taskId: "task-pre-102",
      triggeredAt: previewTime(-28),
    }),
  ],
  "autopilot-ci-watch": [
    previewRun({
      id: "run-ci-watch",
      autopilotId: "autopilot-ci-watch",
      triggerId: "trigger-ci-watch",
      source: "schedule",
      status: "running",
      issueNumber: "104",
      taskId: "task-pre-104",
      triggeredAt: previewTime(-241),
    }),
  ],
  "autopilot-weekly-summary": [
    previewRun({
      id: "run-weekly-skipped",
      autopilotId: "autopilot-weekly-summary",
      triggerId: "trigger-weekly-summary",
      source: "schedule",
      status: "skipped",
      triggeredAt: previewTime(-10080),
      failureReason: "Agent runtime is offline.",
      reasonCode: "agent_runtime_required",
    }),
  ],
};

function localizePreviewIssue(issue, copy = DEFAULT_PREVIEW_COPY) {
  const fixture = copy.issues?.[String(issue.number)];
  if (!fixture) return issue;
  return {
    ...issue,
    title: fixture.title ?? issue.title,
    description: fixture.description ?? issue.description,
  };
}

function localizedPreviewIssues(copy = DEFAULT_PREVIEW_COPY) {
  return PREVIEW_ISSUES.map((issue) => localizePreviewIssue(issue, copy));
}

function localizePreviewAgent(agent, copy = DEFAULT_PREVIEW_COPY) {
  const fixture = copy.agents?.[agent.id];
  if (!fixture) return agent;
  return {
    ...agent,
    name: fixture.name ?? agent.name,
    description: fixture.description ?? agent.description,
    instructions: fixture.instructions ?? agent.instructions,
  };
}

function localizePreviewRuntime(runtime, copy = DEFAULT_PREVIEW_COPY) {
  const name = copy.runtimes?.[runtime.id];
  const deviceInfo = copy.runtime_device_info?.[runtime.id];
  return {
    ...runtime,
    ...(name ? { name } : {}),
    ...(deviceInfo ? { device_info: deviceInfo } : {}),
  };
}

function localizePreviewTask(task, copy = DEFAULT_PREVIEW_COPY) {
  const fixture = copy.tasks?.[task.id];
  if (!fixture) return task;
  const result =
    fixture.result_summary && task.result && typeof task.result === "object"
      ? { ...task.result, summary: fixture.result_summary }
      : task.result;
  return {
    ...task,
    ...(fixture.trigger_summary ? { trigger_summary: fixture.trigger_summary } : {}),
    ...(fixture.handoff_note ? { handoff_note: fixture.handoff_note } : {}),
    ...(result ? { result } : {}),
  };
}

function localizePreviewAutopilot(autopilot, copy = DEFAULT_PREVIEW_COPY) {
  const fixture = copy.autopilots?.[autopilot.id];
  if (!fixture) return autopilot;
  return {
    ...autopilot,
    title: fixture.title ?? autopilot.title,
    description: fixture.description ?? autopilot.description,
    ...(fixture.issue_title_template
      ? { issue_title_template: fixture.issue_title_template }
      : {}),
  };
}

function localizePreviewTrigger(trigger, copy = DEFAULT_PREVIEW_COPY) {
  return copy.trigger_label
    ? { ...trigger, label: copy.trigger_label }
    : trigger;
}

function localizePreviewRun(run, copy = DEFAULT_PREVIEW_COPY) {
  const resultSummary = copy.run_results?.[run.id];
  const failureReason = copy.run_failures?.[run.id];
  const result =
    resultSummary && run.result && typeof run.result === "object"
      ? { ...run.result, summary: resultSummary }
      : run.result;
  return {
    ...run,
    ...(failureReason ? { failure_reason: failureReason } : {}),
    ...(result ? { result } : {}),
  };
}

function sortPreviewRuns(runs) {
  return [...runs].sort(
    (left, right) => Date.parse(right.triggered_at) - Date.parse(left.triggered_at),
  );
}

function previewAutopilotWithLatestRun(autopilot) {
  const latestRun = sortPreviewRuns(PREVIEW_RUNS[autopilot.id] ?? [])[0];
  return latestRun
    ? {
        ...autopilot,
        last_run_at: latestRun.triggered_at,
        last_run_status: latestRun.status,
      }
    : autopilot;
}

function isActiveTask(task) {
  return task.status === "queued" ||
    task.status === "dispatched" ||
    task.status === "waiting_local_directory" ||
    task.status === "running";
}

function previewRunningTasksByAgent() {
  const byAgent = new Map();
  for (const task of PREVIEW_TASKS) {
    if (task.status !== "running") continue;
    const current = byAgent.get(task.agent_id) ?? {
      issue_ids: new Set(),
      running_task_count: 0,
    };
    current.running_task_count += 1;
    if (task.issue_id) current.issue_ids.add(task.issue_id);
    byAgent.set(task.agent_id, current);
  }
  return byAgent;
}

function previewWorkingAgents(copy = DEFAULT_PREVIEW_COPY) {
  const byAgent = previewRunningTasksByAgent();
  return PREVIEW_DIRECTORY_AGENTS
    .map((agent) => {
      const current = byAgent.get(agent.id);
      if (!current) return null;
      return {
        id: agent.id,
        name: copy.agents?.[agent.id]?.name ?? agent.name,
        avatar_url: agent.avatar_url,
        running_task_count: current.running_task_count,
        issue_ids: [...current.issue_ids],
      };
    })
    .filter(Boolean);
}

function findPreviewAgent(value) {
  const id = decodeURIComponent(value);
  return PREVIEW_DIRECTORY_AGENTS.find((agent) => agent.id === id) ?? null;
}

function findPreviewAutopilot(value) {
  const id = decodeURIComponent(value);
  return PREVIEW_AUTOPILOTS.find((autopilot) => autopilot.id === id) ?? null;
}

function previewAutopilotDetail(autopilot, copy = DEFAULT_PREVIEW_COPY) {
  return {
    autopilot: localizePreviewAutopilot(
      previewAutopilotWithLatestRun(autopilot),
      copy,
    ),
    triggers: (PREVIEW_TRIGGERS[autopilot.id] ?? []).map((trigger) =>
      localizePreviewTrigger(trigger, copy),
    ),
    collaborators: [],
  };
}

function listPreviewIssues(url, copy = DEFAULT_PREVIEW_COPY) {
  let issues = localizedPreviewIssues(copy);
  const search = url.searchParams.get("q")?.trim().toLowerCase();
  const statuses = (url.searchParams.get("statuses") ?? "")
    .split(",")
    .map((status) => status.trim())
    .filter(Boolean);
  const status = url.searchParams.get("status");
  const priorities = (url.searchParams.get("priorities") ?? "")
    .split(",")
    .map((priority) => priority.trim())
    .filter(Boolean);
  const priority = url.searchParams.get("priority");
  const assigneeTypes = (url.searchParams.get("assignee_types") ?? "")
    .split(",")
    .map((type) => type.trim())
    .filter(Boolean);
  const requestedIds = url.searchParams.has("ids")
    ? (url.searchParams.get("ids") ?? "").split(",").filter(Boolean)
    : null;

  if (search) {
    issues = issues.filter((issue) =>
      `${issue.identifier} ${issue.title} ${issue.description ?? ""}`
        .toLowerCase()
        .includes(search),
    );
  }
  if (statuses.length > 0) {
    issues = issues.filter((issue) => statuses.includes(issue.status) || statuses.includes(categoryOf(issue)));
  } else if (status) {
    issues = issues.filter((issue) => issue.status === status || categoryOf(issue) === status);
  }
  if (priorities.length > 0) {
    issues = issues.filter((issue) => priorities.includes(issue.priority));
  } else if (priority) {
    issues = issues.filter((issue) => issue.priority === priority);
  }
  if (assigneeTypes.length > 0) {
    issues = issues.filter((issue) => assigneeTypes.includes(issue.assignee_type));
  }
  if (requestedIds !== null) {
    issues = issues.filter((issue) => requestedIds.includes(issue.id));
  }
  const total = issues.length;
  const offset = Number(url.searchParams.get("offset") ?? 0) || 0;
  const limit = Number(url.searchParams.get("limit") ?? 50) || 50;
  return { issues: issues.slice(offset, offset + limit), total };
}

const preferences = new Map();

function issueId(number) {
  return `00000000-0000-4000-8000-000000000${number}`;
}

function previewIssue(
  number,
  status,
  title,
  description,
  priority,
  assignee = "member",
  reviewer = null,
  createdAt = NOW,
) {
  const id = issueId(number);
  const isAgent = assignee === "agent";
  return {
    id,
    workspace_id: WORKSPACE_ID,
    number: Number(number),
    identifier: `PRE-${number}`,
    title,
    description,
    status,
    status_category: status,
    priority,
    assignee_type: isAgent ? "agent" : "member",
    assignee_id: isAgent ? PREVIEW_AGENT_ID : PREVIEW_USER_ID,
    ...(reviewer
      ? { reviewer_type: reviewer.type, reviewer_id: reviewer.id }
      : {}),
    creator_type: "member",
    creator_id: PREVIEW_USER_ID,
    parent_issue_id: null,
    project_id: null,
    position: Number(number),
    stage: null,
    start_date: null,
    due_date: null,
    metadata: {},
    properties: {},
    labels: [],
    created_at: createdAt,
    updated_at: NOW,
  };
}

function json(res, body, status = 200) {
  res.statusCode = status;
  res.setHeader("Content-Type", "application/json; charset=utf-8");
  res.end(JSON.stringify(body));
  return true;
}

function readBody(req) {
  return new Promise((resolve, reject) => {
    let body = "";
    req.setEncoding("utf8");
    req.on("data", (chunk) => {
      body += chunk;
      if (body.length > 1_000_000) {
        reject(new Error("preview request body is too large"));
        req.destroy();
      }
    });
    req.on("end", () => {
      if (!body) {
        resolve({});
        return;
      }
      try {
        resolve(JSON.parse(body));
      } catch {
        reject(new Error("preview request body is not valid JSON"));
      }
    });
    req.on("error", reject);
  });
}

function categoryOf(issue) {
  return issue.status_category ?? issue.status;
}

function actorMatches(issue, actors, type) {
  if (!Array.isArray(actors) || actors.length === 0) return true;
  return actors.some(
    (actor) => actor?.type === issue[`${type}_type`] && actor?.id === issue[`${type}_id`],
  );
}

function matchesIssue(issue, query = {}, { ignoreStatus = false, ignoreWorking = false } = {}) {
  const scope = query.scope ?? {};
  if (Array.isArray(scope.assignee_types) && !scope.assignee_types.includes(issue.assignee_type)) {
    return false;
  }
  if (scope.kind === "project" && scope.project_id !== issue.project_id) {
    return false;
  }
  if (scope.kind === "assignee" && !actorMatches(issue, [scope.actor], "assignee")) {
    return false;
  }
  if (scope.kind === "creator" && !actorMatches(issue, [scope.actor], "creator")) {
    return false;
  }

  const filters = query.filters ?? {};
  if (!ignoreStatus && Array.isArray(filters.statuses) && filters.statuses.length > 0) {
    if (!filters.statuses.some((status) => status === issue.status || status === categoryOf(issue))) {
      return false;
    }
  }
  if (Array.isArray(filters.priorities) && filters.priorities.length > 0 && !filters.priorities.includes(issue.priority)) {
    return false;
  }
  if (!actorMatches(issue, filters.assignees, "assignee")) {
    if (!(filters.include_no_assignee && issue.assignee_id === null)) return false;
  }
  if (!actorMatches(issue, filters.creators, "creator")) return false;
  if (Array.isArray(filters.project_ids) && filters.project_ids.length > 0 && !filters.project_ids.includes(issue.project_id)) {
    if (!(filters.include_no_project && issue.project_id === null)) return false;
  }
  if (Array.isArray(filters.label_ids) && filters.label_ids.length > 0) {
    const labels = new Set((issue.labels ?? []).map((label) => label.id));
    if (!filters.label_ids.some((labelId) => labels.has(labelId))) return false;
  }
  if (!ignoreWorking && Array.isArray(filters.working_issue_ids)) {
    if (!filters.working_issue_ids.includes(issue.id)) return false;
  }
  if (query.search) {
    const term = String(query.search).trim().toLowerCase();
    if (term && !`${issue.identifier} ${issue.title} ${issue.description ?? ""}`.toLowerCase().includes(term)) {
      return false;
    }
  }
  return true;
}

function filteredIssues(query, options, copy = DEFAULT_PREVIEW_COPY) {
  return localizedPreviewIssues(copy).filter((issue) => matchesIssue(issue, query, options));
}

function sortIssues(issues, query = {}) {
  const field = query.sort?.field ?? "position";
  const direction = query.sort?.direction === "desc" ? -1 : 1;
  return [...issues].sort((a, b) => {
    const left = field === "title" ? a.title : field === "priority" ? a.priority : a[field] ?? "";
    const right = field === "title" ? b.title : field === "priority" ? b.priority : b[field] ?? "";
    return String(left).localeCompare(String(right), undefined, { numeric: true }) * direction;
  });
}

function facetValues(kind, issues) {
  const counts = new Map();
  const add = (key) => counts.set(key, (counts.get(key) ?? 0) + 1);
  for (const issue of issues) {
    if (kind === "status") add(issue.status);
    else if (kind === "priority") add(issue.priority);
    else if (kind === "assignee") add(`${issue.assignee_type}:${issue.assignee_id}`);
    else if (kind === "creator") add(`${issue.creator_type}:${issue.creator_id}`);
    else if (kind === "project") add(issue.project_id ?? "__none__");
    else if (kind === "label") for (const label of issue.labels ?? []) add(label.id);
  }
  return [...counts].map(([key, count]) => ({ key, count }));
}

function tableFacets(body, copy = DEFAULT_PREVIEW_COPY) {
  const requested = Array.isArray(body.facets) ? body.facets : [];
  const facets = requested.map((request) => {
    if (request.kind === "working_agents") {
      const issues = filteredIssues(body.query, { ignoreWorking: true }, copy);
      const runningByAgent = previewRunningTasksByAgent();
      const values = PREVIEW_DIRECTORY_AGENTS.flatMap((agent) => {
        const running = runningByAgent.get(agent.id);
        if (!running) return [];
        const count = issues.filter((issue) => running.issue_ids.has(issue.id)).length;
        return count > 0 ? [{ key: agent.id, count }] : [];
      });
      return {
        kind: request.kind,
        values,
      };
    }
    const issues = request.kind === "status"
      ? filteredIssues(body.query, { ignoreStatus: true }, copy)
      : filteredIssues(body.query, undefined, copy);
    return {
      kind: request.kind,
      ...(request.kind === "property" ? { property_id: request.property_id } : {}),
      values: request.kind === "property" ? [] : facetValues(request.kind, issues),
    };
  });
  return {
    query_fingerprint: JSON.stringify(body.query ?? {}),
    total: filteredIssues(body.query, undefined, copy).length,
    facets,
  };
}

function groupKeyForIssue(issue, group) {
  if (group?.kind === "assignee") return issue.assignee_id ? `assignee:${issue.assignee_type}:${issue.assignee_id}` : "assignee:none";
  if (group?.kind === "project") return issue.project_id ? `project:${issue.project_id}` : "project:none";
  return `status:${categoryOf(issue)}`;
}

function groupValueForIssue(issue, group) {
  if (group?.kind === "assignee") {
    return { kind: "assignee", actor: issue.assignee_id ? { type: issue.assignee_type, id: issue.assignee_id } : null };
  }
  if (group?.kind === "project") return { kind: "project", project_id: issue.project_id };
  return { kind: "status", status: categoryOf(issue) };
}

function tableGroups(body, copy = DEFAULT_PREVIEW_COPY) {
  const issues = filteredIssues(body.query, undefined, copy);
  const grouped = new Map();
  for (const issue of issues) {
    const key = groupKeyForIssue(issue, body.group);
    const current = grouped.get(key);
    if (current) current.count += 1;
    else grouped.set(key, { key, value: groupValueForIssue(issue, body.group), count: 1 });
  }
  return {
    query_fingerprint: JSON.stringify(body.query ?? {}),
    total: issues.length,
    groups: [...grouped.values()],
    next_cursor: null,
  };
}

function tableRows(body, copy = DEFAULT_PREVIEW_COPY) {
  let issues = filteredIssues(body.query, undefined, copy);
  const groupKey = body.group_key;
  if (groupKey?.startsWith("status:")) {
    const category = groupKey.slice("status:".length);
    issues = issues.filter((issue) => categoryOf(issue) === category);
  } else if (groupKey?.startsWith("assignee:")) {
    const expected = groupKey.slice("assignee:".length);
    issues = issues.filter(
      (issue) =>
        `${issue.assignee_type}:${issue.assignee_id}` === expected ||
        (expected === "none" && issue.assignee_id === null),
    );
  } else if (groupKey?.startsWith("project:")) {
    const expected = groupKey.slice("project:".length);
    issues = issues.filter((issue) => (issue.project_id ?? "none") === expected);
  }
  issues = sortIssues(issues, body.query);
  const limit = Number(body.page?.limit) || 50;
  const offset = body.page?.cursor ? Number(body.page.cursor) || 0 : 0;
  const page = issues.slice(offset, offset + limit);
  return {
    query_fingerprint: JSON.stringify(body.query ?? {}),
    group_key: groupKey ?? null,
    parent_id: body.parent_id ?? null,
    total: issues.length,
    rows: page.map((issue) => ({ issue, direct_child_count: 0 })),
    branch_total: issues.length,
    next_cursor: offset + limit < issues.length ? String(offset + limit) : null,
  };
}

function preferenceKey(url) {
  return `${url.searchParams.get("scope_type") ?? "workspace"}:${url.searchParams.get("scope_id") ?? ""}`;
}

function findPreviewIssue(value) {
  const id = decodeURIComponent(value);
  return PREVIEW_ISSUES.find(
    (issue) => issue.id === id || issue.identifier === id,
  );
}

export async function handlePreviewRequest(req, res) {
  const url = new URL(req.url ?? "/", "http://127.0.0.1");
  const method = req.method ?? "GET";
  const path = url.pathname;
  const copy = previewCopyForRequest(req);

  if (method === "GET" && path === "/api/workspaces") return json(res, [PREVIEW_WORKSPACE]);
  if (method === "GET" && path === `/api/workspaces/${WORKSPACE_ID}/members`) return json(res, PREVIEW_MEMBERS);
  if (method === "GET" && path === `/api/workspaces/${WORKSPACE_ID}/lark/installations`) {
    return json(res, PREVIEW_EMPTY_INSTALLATIONS);
  }
  if (method === "GET" && path === `/api/workspaces/${WORKSPACE_ID}/slack/installations`) {
    return json(res, PREVIEW_EMPTY_INSTALLATIONS);
  }
  if (method === "GET" && path === `/api/workspaces/${WORKSPACE_ID}/dingtalk/installations`) {
    return json(res, PREVIEW_EMPTY_INSTALLATIONS);
  }
  if (method === "GET" && path === `/api/workspaces/${WORKSPACE_ID}/dingtalk/group-routes`) {
    return json(res, PREVIEW_EMPTY_DINGTALK_GROUP_ROUTES);
  }
  if (method === "GET" && path === `/api/workspaces/${WORKSPACE_ID}/wecom/installations`) {
    return json(res, PREVIEW_EMPTY_INSTALLATIONS);
  }
  if (method === "GET" && path === `/api/workspaces/${WORKSPACE_ID}/telegram/installations`) {
    return json(res, PREVIEW_EMPTY_INSTALLATIONS);
  }
  if (method === "GET" && path === `/api/workspaces/${WORKSPACE_ID}/weixin/installations`) {
    return json(res, PREVIEW_EMPTY_INSTALLATIONS);
  }
  if (method === "GET" && path === `/api/workspaces/${WORKSPACE_ID}/runtime-profiles`) {
    return json(res, PREVIEW_EMPTY_RUNTIME_PROFILES);
  }
  if (method === "GET" && path === `/api/workspaces/${WORKSPACE_ID}/plugins`) {
    return json(res, PREVIEW_EMPTY_PLUGINS);
  }
  const workspaceMcpServers = /^\/api\/workspaces\/([^/]+)\/mcp-servers$/.exec(path);
  if (method === "GET" && workspaceMcpServers) {
    return workspaceMcpServers[1] === WORKSPACE_ID
      ? json(res, PREVIEW_EMPTY_MCP_SERVERS)
      : json(res, { error: "Preview workspace not found" }, 404);
  }
  const chatMessagesPage = /^\/api\/chat\/sessions\/([^/]+)\/messages\/page$/.exec(path);
  if (method === "GET" && chatMessagesPage) {
    return decodeURIComponent(chatMessagesPage[1]) === PREVIEW_CHAT_SESSION_ID
      ? json(res, previewChatMessagesPage(url))
      : json(res, { error: "Preview chat session not found" }, 404);
  }
  const chatMessages = /^\/api\/chat\/sessions\/([^/]+)\/messages$/.exec(path);
  if (method === "GET" && chatMessages) {
    return decodeURIComponent(chatMessages[1]) === PREVIEW_CHAT_SESSION_ID
      ? json(res, [])
      : json(res, { error: "Preview chat session not found" }, 404);
  }
  const chatPendingTask = /^\/api\/chat\/sessions\/([^/]+)\/pending-task$/.exec(path);
  if (method === "GET" && chatPendingTask) {
    return decodeURIComponent(chatPendingTask[1]) === PREVIEW_CHAT_SESSION_ID
      ? json(res, PREVIEW_EMPTY_CHAT_PENDING_TASK)
      : json(res, { error: "Preview chat session not found" }, 404);
  }
  if (method === "GET" && path === "/api/agents") {
    return json(res, PREVIEW_DIRECTORY_AGENTS.map((agent) => localizePreviewAgent(agent, copy)));
  }
  const agentTasks = /^\/api\/agents\/([^/]+)\/tasks$/.exec(path);
  if (method === "GET" && agentTasks) {
    return findPreviewAgent(agentTasks[1])
      ? json(res, PREVIEW_TASKS
        .filter((task) => task.agent_id === decodeURIComponent(agentTasks[1]))
        .map((task) => localizePreviewTask(task, copy)))
      : json(res, { error: "Preview agent not found" }, 404);
  }
  const agentMcpServers = /^\/api\/agents\/([^/]+)\/mcp-servers$/.exec(path);
  if (method === "GET" && agentMcpServers) {
    return findPreviewAgent(agentMcpServers[1])
      ? json(res, PREVIEW_EMPTY_MCP_SERVERS)
      : json(res, { error: "Preview agent not found" }, 404);
  }
  if (method === "GET" && path.startsWith("/api/agents/")) {
    const agent = findPreviewAgent(path.slice("/api/agents/".length));
    return agent
      ? json(res, localizePreviewAgent(agent, copy))
      : json(res, { error: "Preview agent not found" }, 404);
  }
  if (method === "GET" && path === "/api/runtimes") {
    return json(res, PREVIEW_RUNTIMES.map((runtime) => localizePreviewRuntime(runtime, copy)));
  }
  const runtimeModels = /^\/api\/runtimes\/([^/]+)\/models$/.exec(path);
  if (method === "POST" && runtimeModels) {
    const runtimeId = decodeURIComponent(runtimeModels[1]);
    return PREVIEW_RUNTIMES.some((runtime) => runtime.id === runtimeId)
      ? json(res, previewRuntimeModels(runtimeId))
      : json(res, { error: "Preview runtime not found" }, 404);
  }
  const runtimeUsage = /^\/api\/runtimes\/([^/]+)\/usage$/.exec(path);
  if (method === "GET" && runtimeUsage) {
    const runtimeId = decodeURIComponent(runtimeUsage[1]);
    return PREVIEW_RUNTIMES.some((runtime) => runtime.id === runtimeId)
      ? json(res, [])
      : json(res, { error: "Preview runtime not found" }, 404);
  }
  const runtimeLocalSkills = /^\/api\/runtimes\/([^/]+)\/local-skills$/.exec(path);
  if (method === "POST" && runtimeLocalSkills) {
    const runtimeId = decodeURIComponent(runtimeLocalSkills[1]);
    return PREVIEW_RUNTIMES.some((runtime) => runtime.id === runtimeId)
      ? json(res, previewRuntimeLocalSkills(runtimeId))
      : json(res, { error: "Preview runtime not found" }, 404);
  }
  if (method === "GET" && path === "/api/squads") return json(res, []);
  if (method === "GET" && path === "/api/projects") return json(res, { projects: [], total: 0 });
  if (method === "GET" && path === "/api/properties") return json(res, { properties: [], total: 0 });
  if (method === "GET" && path === "/api/labels") return json(res, { labels: [], total: 0 });
  if (method === "GET" && path === "/api/pins") return json(res, []);
  if (method === "GET" && path === "/api/issue-views") return json(res, []);
  if (method === "GET" && path === "/api/issue-view-preferences") {
    const key = preferenceKey(url);
    return json(res, preferences.get(key) ?? {
      scope_type: url.searchParams.get("scope_type") ?? "workspace",
      scope_id: url.searchParams.get("scope_id"),
      prefs: { hidden: [], order: [] },
      updated_at: NOW,
    });
  }
  if (method === "PUT" && path === "/api/issue-view-preferences") {
    const body = await readBody(req);
    const key = `${body.scope_type ?? "workspace"}:${body.scope_id ?? ""}`;
    const value = {
      scope_type: body.scope_type ?? "workspace",
      scope_id: body.scope_id ?? null,
      prefs: body.prefs ?? { hidden: [], order: [] },
      updated_at: NOW,
    };
    preferences.set(key, value);
    return json(res, value);
  }
  if (method === "GET" && path === "/api/issue-statuses") {
    return json(res, {
      statuses: STATUS_CATEGORIES.map((category, index) => ({
        id: `status-${category}`,
        workspace_id: WORKSPACE_ID,
        key: category,
        name: category,
        description: "",
        category,
        color: "#6b7280",
        is_system: true,
        position: index,
        archived_at: null,
        created_at: NOW,
        updated_at: NOW,
      })),
      categories: STATUS_CATEGORIES,
      total: STATUS_CATEGORIES.length,
    });
  }
  if (method === "GET" && path === "/api/working-agents") return json(res, previewWorkingAgents(copy));
  if (method === "GET" && path === "/api/agent-task-snapshot") {
    return json(res, PREVIEW_TASKS.map((task) => localizePreviewTask(task, copy)));
  }
  if (method === "GET" && path === "/api/agent-activity-30d") return json(res, PREVIEW_ACTIVITY);
  if (method === "GET" && path === "/api/agent-run-counts") return json(res, PREVIEW_RUN_COUNTS);
  if (method === "GET" && path === "/api/assignee-frequency") return json(res, []);
  if (method === "GET" && path === "/api/quick-actions") {
    return json(res, { quick_actions: [], total: 0 });
  }
  if (method === "GET" && path === "/api/autopilots/usage") {
    return json(res, {
      action: "off",
      used: null,
      reserved: null,
      limit: null,
      period_start: null,
      period_end: null,
      reset_at: null,
      blocked_counts: null,
    });
  }
  if (method === "GET" && path === "/api/autopilots/cron-preview") {
    const expression = url.searchParams.get("expr") ?? "*/15 * * * *";
    const timeZone = url.searchParams.get("tz") ?? PREVIEW_TIMEZONE;
    try {
      const nextRuns = [];
      let after = PREVIEW_SESSION_STARTED_AT;
      for (let index = 0; index < 3; index += 1) {
        const nextRun = previewNextCronOccurrence(expression, timeZone, after);
        nextRuns.push(nextRun);
        after = Date.parse(nextRun);
      }
      return json(res, { next_runs: nextRuns });
    } catch (error) {
      return json(res, {
        error: error instanceof Error ? error.message : "Invalid preview cron",
      }, 400);
    }
  }
  if (method === "GET" && path === "/api/autopilots") {
    return json(res, {
      autopilots: PREVIEW_AUTOPILOTS.map((autopilot) =>
        localizePreviewAutopilot(previewAutopilotWithLatestRun(autopilot), copy),
      ),
      total: PREVIEW_AUTOPILOTS.length,
      // Preview intentionally cannot create or mutate product data. This is
      // a collection capability, not a projection of per-row can_write.
      can_create: false,
    });
  }
  const autopilotRunDetail = /^\/api\/autopilots\/([^/]+)\/runs\/([^/]+)$/.exec(path);
  if (method === "GET" && autopilotRunDetail) {
    const runs = PREVIEW_RUNS[decodeURIComponent(autopilotRunDetail[1])] ?? [];
    const run = runs.find((item) => item.id === decodeURIComponent(autopilotRunDetail[2]));
    return run
      ? json(res, localizePreviewRun(run, copy))
      : json(res, { error: "Preview run not found" }, 404);
  }
  const autopilotRuns = /^\/api\/autopilots\/([^/]+)\/runs$/.exec(path);
  if (method === "GET" && autopilotRuns) {
    const autopilot = findPreviewAutopilot(autopilotRuns[1]);
    if (!autopilot) return json(res, { error: "Preview autopilot not found" }, 404);
    const runs = sortPreviewRuns(PREVIEW_RUNS[autopilot.id] ?? []);
    return json(res, {
      runs: runs.map((run) => localizePreviewRun(run, copy)),
      total: runs.length,
    });
  }
  const autopilotDeliveries = /^\/api\/autopilots\/([^/]+)\/deliveries(?:\/([^/]+))?$/.exec(path);
  if (method === "GET" && autopilotDeliveries) {
    if (!findPreviewAutopilot(autopilotDeliveries[1])) {
      return json(res, { error: "Preview autopilot not found" }, 404);
    }
    return autopilotDeliveries[2]
      ? json(res, { error: "Preview delivery not found" }, 404)
      : json(res, { deliveries: [], total: 0 });
  }
  const autopilotDetail = /^\/api\/autopilots\/([^/]+)$/.exec(path);
  if (method === "GET" && autopilotDetail) {
    const autopilot = findPreviewAutopilot(autopilotDetail[1]);
    return autopilot
      ? json(res, previewAutopilotDetail(autopilot, copy))
      : json(res, { error: "Preview autopilot not found" }, 404);
  }
  if (method === "GET" && path === "/api/issues/child-progress") return json(res, { progress: [] });
  const taskMessagesResource = /^\/api\/tasks\/([^/]+)\/messages$/.exec(path);
  if (method === "GET" && taskMessagesResource) {
    const taskId = decodeURIComponent(taskMessagesResource[1]);
    return PREVIEW_TASKS.some((task) => task.id === taskId)
      ? json(res, [])
      : json(res, { error: "Preview task not found" }, 404);
  }
  if (method === "GET" && path === "/api/issues") return json(res, listPreviewIssues(url, copy));
  if (method === "POST" && path === "/api/issues/query") {
    const body = await readBody(req);
    const query = new URL("http://127.0.0.1/api/issues");
    for (const [key, value] of Object.entries(body ?? {})) {
      if (value === undefined || value === null) continue;
      query.searchParams.set(key, Array.isArray(value) ? value.join(",") : String(value));
    }
    return json(res, listPreviewIssues(query, copy));
  }
  const commentsResource = /^\/api\/issues\/([^/]+)\/comments$/.exec(path);
  if (method === "GET" && commentsResource) {
    return findPreviewIssue(commentsResource[1])
      ? json(res, [])
      : json(res, { error: "Preview issue not found" }, 404);
  }
  const issueResource = /^\/api\/issues\/([^/]+)\/(timeline|subscribers|attachments|labels|task-runs|pull-requests|children)$/.exec(path);
  if (method === "GET" && issueResource) {
    if (!findPreviewIssue(issueResource[1])) {
      return json(res, { error: "Preview issue not found" }, 404);
    }
    switch (issueResource[2]) {
      case "timeline":
      case "subscribers":
      case "attachments":
        return json(res, []);
      case "task-runs":
        return json(res, PREVIEW_TASKS
          .filter((task) => task.issue_id === findPreviewIssue(issueResource[1])?.id)
          .map((task) => localizePreviewTask(task, copy)));
      case "labels":
        return json(res, { labels: [] });
      case "pull-requests":
        return json(res, { pull_requests: [] });
      case "children":
        return json(res, { issues: [] });
    }
  }
  const activeTaskIssue = /^\/api\/issues\/([^/]+)\/active-task$/.exec(path);
  if (method === "GET" && activeTaskIssue) {
    const issue = findPreviewIssue(activeTaskIssue[1]);
    if (!issue) return json(res, { error: "Preview issue not found" }, 404);
    return json(res, {
      tasks: PREVIEW_TASKS
        .filter((task) => task.issue_id === issue.id && isActiveTask(task))
        .map((task) => localizePreviewTask(task, copy)),
    });
  }
  const issueUsage = /^\/api\/issues\/([^/]+)\/usage$/.exec(path);
  if (method === "GET" && issueUsage) {
    const issue = findPreviewIssue(issueUsage[1]);
    if (!issue) return json(res, { error: "Preview issue not found" }, 404);
    const tasks = PREVIEW_TASKS.filter((task) => task.issue_id === issue.id);
    return json(res, {
      total_input_tokens: 0,
      total_output_tokens: 0,
      total_cache_read_tokens: 0,
      total_cache_write_tokens: 0,
      task_count: tasks.length,
    });
  }
  if (method === "GET" && path.startsWith("/api/issues/")) {
    const issue = findPreviewIssue(path.slice("/api/issues/".length));
    return issue
      ? json(res, localizePreviewIssue(issue, copy))
      : json(res, { error: "Preview issue not found" }, 404);
  }
  if (method === "POST" && path === "/api/issues/table/facets") {
    return json(res, tableFacets(await readBody(req), copy));
  }
  if (method === "POST" && path === "/api/issues/table/groups") {
    return json(res, tableGroups(await readBody(req), copy));
  }
  if (method === "POST" && path === "/api/issues/table/rows") {
    return json(res, tableRows(await readBody(req), copy));
  }

  return false;
}

/**
 * The browser host's local API is deliberately an HTTP boundary, not a second
 * React page. It supplies the same response contracts that the shared issue
 * surface consumes, so Vite HMR exercises the production renderer. It is only
 * installed by vite.web.config.mjs; Electron continues to use its configured
 * real backend. Product writes fall through to Vite and remain visible as
 * unavailable rather than being reported as successful mutations. The one
 * handled PUT only stores issue-view preferences in this process for local
 * browsing; it never persists product data.
 */
export function previewApiPlugin() {
  return {
    name: "patchbay-local-preview-api",
    configureServer(server) {
      server.middlewares.use(async (req, res, next) => {
        if (!req.url?.startsWith("/api/")) {
          next();
          return;
        }
        try {
          const handled = await handlePreviewRequest(req, res);
          if (!handled && !res.writableEnded) next();
        } catch (error) {
          if (res.writableEnded) return;
          json(res, {
            error: error instanceof Error ? error.message : "Preview API request failed",
          }, 500);
        }
      });
    },
  };
}
