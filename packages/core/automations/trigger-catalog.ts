// Canonical trigger presets for the Cursor-style automation editor.
// IDs are stable API values (`automation_trigger.preset`). Labels are UI-only.

export type AutomationTriggerProvider =
  | "schedule"
  | "github"
  | "slack"
  | "linear"
  | "generic";

export type AutomationTriggerSourceId =
  | "scheduled"
  | "github"
  | "slack"
  | "linear"
  | "webhook";

export interface AutomationTriggerPreset {
  id: string;
  source: AutomationTriggerSourceId;
  provider: AutomationTriggerProvider;
  kind: "schedule" | "webhook";
  // i18n key under automations.presets.<id>
  labelKey: string;
  group?: "core" | "github_only";
}

export interface AutomationTriggerSource {
  id: AutomationTriggerSourceId;
  provider: AutomationTriggerProvider;
  // i18n key under automations.trigger_sources.<id>
  labelKey: string;
  nested: boolean;
}

export const AUTOMATION_TRIGGER_SOURCES: readonly AutomationTriggerSource[] = [
  { id: "scheduled", provider: "schedule", labelKey: "scheduled", nested: false },
  { id: "github", provider: "github", labelKey: "github", nested: true },
  { id: "slack", provider: "slack", labelKey: "slack", nested: true },
  { id: "linear", provider: "linear", labelKey: "linear", nested: true },
  { id: "webhook", provider: "generic", labelKey: "webhook", nested: false },
] as const;

export const AUTOMATION_TRIGGER_PRESETS: readonly AutomationTriggerPreset[] = [
  {
    id: "scheduled",
    source: "scheduled",
    provider: "schedule",
    kind: "schedule",
    labelKey: "scheduled",
  },
  {
    id: "github.draft.opened",
    source: "github",
    provider: "github",
    kind: "webhook",
    labelKey: "github_draft_opened",
    group: "core",
  },
  {
    id: "github.pull_request.opened",
    source: "github",
    provider: "github",
    kind: "webhook",
    labelKey: "github_pr_opened",
    group: "core",
  },
  {
    id: "github.pull_request.pushed",
    source: "github",
    provider: "github",
    kind: "webhook",
    labelKey: "github_pr_pushed",
    group: "core",
  },
  {
    id: "github.pull_request.merged",
    source: "github",
    provider: "github",
    kind: "webhook",
    labelKey: "github_pr_merged",
    group: "core",
  },
  {
    id: "github.push_to_branch",
    source: "github",
    provider: "github",
    kind: "webhook",
    labelKey: "github_push_to_branch",
    group: "core",
  },
  {
    id: "github.pull_request.comment",
    source: "github",
    provider: "github",
    kind: "webhook",
    labelKey: "github_pr_comment",
    group: "core",
  },
  {
    id: "github.pull_request.label_changed",
    source: "github",
    provider: "github",
    kind: "webhook",
    labelKey: "github_pr_label_changed",
    group: "github_only",
  },
  {
    id: "github.issue.label_changed",
    source: "github",
    provider: "github",
    kind: "webhook",
    labelKey: "github_issue_label_changed",
    group: "github_only",
  },
  {
    id: "github.ci_completed",
    source: "github",
    provider: "github",
    kind: "webhook",
    labelKey: "github_ci_completed",
    group: "github_only",
  },
  {
    id: "github.issue.comment",
    source: "github",
    provider: "github",
    kind: "webhook",
    labelKey: "github_issue_comment",
    group: "github_only",
  },
  {
    id: "github.pull_request.review_comment",
    source: "github",
    provider: "github",
    kind: "webhook",
    labelKey: "github_pr_review_comment",
    group: "github_only",
  },
  {
    id: "github.pull_request.review_submitted",
    source: "github",
    provider: "github",
    kind: "webhook",
    labelKey: "github_pr_review_submitted",
    group: "github_only",
  },
  {
    id: "github.pull_request.review_thread",
    source: "github",
    provider: "github",
    kind: "webhook",
    labelKey: "github_pr_review_thread",
    group: "github_only",
  },
  {
    id: "github.workflow_run.completed",
    source: "github",
    provider: "github",
    kind: "webhook",
    labelKey: "github_workflow_run_completed",
    group: "github_only",
  },
  {
    id: "slack.message",
    source: "slack",
    provider: "slack",
    kind: "webhook",
    labelKey: "slack_message",
  },
  {
    id: "slack.reaction",
    source: "slack",
    provider: "slack",
    kind: "webhook",
    labelKey: "slack_reaction",
  },
  {
    id: "slack.channel_created",
    source: "slack",
    provider: "slack",
    kind: "webhook",
    labelKey: "slack_channel_created",
  },
  {
    id: "linear.issue.created",
    source: "linear",
    provider: "linear",
    kind: "webhook",
    labelKey: "linear_issue_created",
  },
  {
    id: "linear.issue.status_changed",
    source: "linear",
    provider: "linear",
    kind: "webhook",
    labelKey: "linear_status_changed",
  },
  {
    id: "linear.cycle.ended",
    source: "linear",
    provider: "linear",
    kind: "webhook",
    labelKey: "linear_cycle_ended",
  },
  {
    id: "webhook.received",
    source: "webhook",
    provider: "generic",
    kind: "webhook",
    labelKey: "webhook_triggered",
  },
] as const;

const PRESET_BY_ID = new Map(
  AUTOMATION_TRIGGER_PRESETS.map((preset) => [preset.id, preset]),
);

export function automationTriggerPreset(id: string | null | undefined): AutomationTriggerPreset | null {
  if (!id) return null;
  return PRESET_BY_ID.get(id) ?? null;
}

export function presetsForSource(source: AutomationTriggerSourceId): AutomationTriggerPreset[] {
  return AUTOMATION_TRIGGER_PRESETS.filter((preset) => preset.source === source);
}

export function searchTriggerCatalog(query: string, labels: Record<string, string> = {}): {
  sources: AutomationTriggerSource[];
  presets: AutomationTriggerPreset[];
} {
  const needle = query.trim().toLowerCase();
  if (!needle) {
    return {
      sources: [...AUTOMATION_TRIGGER_SOURCES],
      presets: [...AUTOMATION_TRIGGER_PRESETS],
    };
  }
  const presets = AUTOMATION_TRIGGER_PRESETS.filter((preset) => {
    return (
      preset.id.toLowerCase().includes(needle) ||
      preset.labelKey.replaceAll("_", " ").includes(needle) ||
      preset.source.includes(needle) ||
      preset.provider.includes(needle)
      || (labels[preset.labelKey]?.toLowerCase().includes(needle) ?? false)
      || (labels[preset.source]?.toLowerCase().includes(needle) ?? false)
    );
  });
  const sourceIds = new Set(presets.map((preset) => preset.source));
  const sources = AUTOMATION_TRIGGER_SOURCES.filter(
    (source) =>
      sourceIds.has(source.id) ||
      source.id.includes(needle) ||
      source.labelKey.includes(needle),
  );
  return { sources, presets };
}

export function isNativeAutomationProvider(provider: string | null | undefined): boolean {
  return provider === "github" || provider === "slack" || provider === "linear";
}

export type AutomationToolId = "memories" | "slack_send" | "mcp";

export interface AutomationToolsConfig {
  // `enabled` is retained only for clients that round-trip legacy payloads;
  // row presence is the execution switch.
  memories?: { enabled?: boolean };
  slack_send?: { enabled?: boolean; channel?: string; installation_id?: string; channel_ids?: string[] };
  mcp_server_ids?: string[];
}

export interface AutomationTriggerConfig {
  channel?: string;
  keyword?: string;
  regex?: string;
  emoji?: string;
  branch?: string;
  label?: string;
  on_failure?: boolean;
  repository?: string;
  repositories?: string[];
  author_scope?: "anyone" | "me" | "specific";
  author_logins?: string[];
  review_state?: string;
  thread_state?: string;
  conclusion?: string;
  installation_id?: string;
  sender_scope?: "anyone" | "authenticated";
  ignore_thread_replies?: boolean;
  completion_reaction?: string;
  team_id?: string;
  project_id?: string;
  status_id?: string;
}

export function parseAutomationTools(raw: unknown): AutomationToolsConfig {
  if (!raw || typeof raw !== "object") return {};
  const value = raw as Record<string, unknown>;
  const tools: AutomationToolsConfig = {};
  if (value.memories && typeof value.memories === "object") {
    const memories = value.memories as { enabled?: unknown };
    tools.memories = typeof memories.enabled === "boolean" ? { enabled: memories.enabled } : {};
  }
  if (value.slack_send && typeof value.slack_send === "object") {
    const slack = value.slack_send as { enabled?: unknown; channel?: unknown; installation_id?: unknown; channel_ids?: unknown };
    tools.slack_send = {
      ...(typeof slack.enabled === "boolean" ? { enabled: slack.enabled } : {}),
      channel: typeof slack.channel === "string" ? slack.channel : undefined,
      installation_id: typeof slack.installation_id === "string" ? slack.installation_id : undefined,
      channel_ids: Array.isArray(slack.channel_ids)
        ? slack.channel_ids.filter((id): id is string => typeof id === "string" && id.length > 0)
        : undefined,
    };
  }
  if (Array.isArray(value.mcp_server_ids)) {
    tools.mcp_server_ids = value.mcp_server_ids.filter(
      (id): id is string => typeof id === "string" && id.length > 0,
    );
  }
  return tools;
}

export function parseAutomationTriggerConfig(raw: unknown): AutomationTriggerConfig {
  if (!raw || typeof raw !== "object") return {};
  const value = raw as Record<string, unknown>;
  return {
    channel: typeof value.channel === "string" ? value.channel : undefined,
    keyword: typeof value.keyword === "string" ? value.keyword : undefined,
    regex: typeof value.regex === "string" ? value.regex : undefined,
    emoji: typeof value.emoji === "string" ? value.emoji : undefined,
    branch: typeof value.branch === "string" ? value.branch : undefined,
    label: typeof value.label === "string" ? value.label : undefined,
    on_failure: typeof value.on_failure === "boolean" ? value.on_failure : undefined,
    repository: typeof value.repository === "string" ? value.repository : undefined,
    repositories: Array.isArray(value.repositories)
      ? value.repositories.filter((item): item is string => typeof item === "string" && item.length > 0)
      : undefined,
    author_scope: value.author_scope === "anyone" || value.author_scope === "me" || value.author_scope === "specific"
      ? value.author_scope
      : undefined,
    author_logins: Array.isArray(value.author_logins)
      ? value.author_logins.filter((item): item is string => typeof item === "string" && item.length > 0)
      : undefined,
    review_state: typeof value.review_state === "string" ? value.review_state : undefined,
    thread_state: typeof value.thread_state === "string" ? value.thread_state : undefined,
    conclusion: typeof value.conclusion === "string" ? value.conclusion : undefined,
    installation_id: typeof value.installation_id === "string" ? value.installation_id : undefined,
    sender_scope: value.sender_scope === "anyone" || value.sender_scope === "authenticated"
      ? value.sender_scope
      : undefined,
    ignore_thread_replies: typeof value.ignore_thread_replies === "boolean" ? value.ignore_thread_replies : undefined,
    completion_reaction: typeof value.completion_reaction === "string" ? value.completion_reaction : undefined,
    team_id: typeof value.team_id === "string" ? value.team_id : undefined,
    project_id: typeof value.project_id === "string" ? value.project_id : undefined,
    status_id: typeof value.status_id === "string" ? value.status_id : undefined,
  };
}

export function settingsPathForTriggerProvider(
  settingsPath: string,
  provider: string | null | undefined,
): string {
  switch (provider) {
    case "github":
      return `${settingsPath}?tab=github`;
    case "slack":
    case "linear":
      return `${settingsPath}?tab=integrations`;
    default:
      return settingsPath;
  }
}
