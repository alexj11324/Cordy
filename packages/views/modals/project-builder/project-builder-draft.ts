export type ProjectBuilderStatus =
  | "planned"
  | "in_progress"
  | "paused"
  | "completed"
  | "cancelled";

export type ProjectBuilderPriority = "urgent" | "high" | "medium" | "low" | "none";

/**
 * AI-only wire shape for a project draft. The main project modal remains the
 * owner of the create mutation; this shape is the assistant's reviewable
 * proposal and is intentionally kept beside the project-builder UI.
 */
export interface ProjectBuilderDraft {
  title: string;
  summary: string;
  description: string;
  icon?: string | null;
  status: ProjectBuilderStatus;
  priority: ProjectBuilderPriority;
  lead_type?: "member" | "agent" | null;
  lead_id?: string | null;
  start_date?: string | null;
  due_date?: string | null;
  member_ids: string[];
  label_ids: string[];
  dependency_ids: string[];
}

export interface ProjectBuilderCatalogItem {
  id: string;
  name: string;
}

export interface ProjectBuilderCatalogs {
  members: readonly ProjectBuilderCatalogItem[];
  agents: readonly ProjectBuilderCatalogItem[];
  labels: readonly ProjectBuilderCatalogItem[];
  projects: readonly ProjectBuilderCatalogItem[];
}

export interface ProjectBuilderDraftPayload {
  title?: unknown;
  summary?: unknown;
  description?: unknown;
  icon?: unknown;
  status?: unknown;
  priority?: unknown;
  lead_type?: unknown;
  lead_id?: unknown;
  start_date?: unknown;
  due_date?: unknown;
  member_ids?: unknown;
  label_ids?: unknown;
  dependency_ids?: unknown;
}

export const PROJECT_BUILDER_INPUT_PREFIX = "ORVILO_PROJECT_BUILDER_INPUT\n";

/**
 * Product instructions carried with every project-builder turn. Project
 * builder conversations use a normal user-selected agent, so the protocol
 * cannot rely on a hidden carrier agent's system prompt to describe the
 * response contract.
 */
export const PROJECT_BUILDER_RESPONSE_INSTRUCTIONS = `You are helping the user create one project in Orvilo. Use the current draft and the supplied workspace catalogs as context, and propose edits without creating or updating resources yourself.

Every response MUST end with exactly one <project_draft> JSON block using this shape:
<project_draft>{"title":"","summary":"","description":"","icon":null,"status":"planned","priority":"none","lead_type":null,"lead_id":null,"start_date":null,"due_date":null,"member_ids":[],"label_ids":[],"dependency_ids":[]}</project_draft>

Rules:
- Keep the natural-language response concise and ask only questions that materially change the project. Make a reasonable proposal immediately when the request is clear.
- Preserve current draft fields unless the user asks to change them.
- The JSON must be valid compact JSON on one physical line. Escape line breaks inside JSON strings as \\n. Never put a literal newline inside a JSON string.
- title, summary, and description are plain project text; status must be planned, in_progress, paused, completed, or cancelled; priority must be urgent, high, medium, low, or none.
- member_ids, label_ids, and dependency_ids may contain only IDs from the supplied catalogs. lead_type is member or agent and lead_id must match the corresponding catalog, or both may be null.
- Do not request, expose, or place secrets, tokens, passwords, or environment-variable values in the draft.
- Do not claim that the project has been created. The user must review and confirm the proposal in the project form.`;

const PROJECT_DRAFT_TAG = /<project_draft>([\s\S]*?)<\/project_draft>/g;
const PROJECT_STATUS = new Set<ProjectBuilderStatus>([
  "planned",
  "in_progress",
  "paused",
  "completed",
  "cancelled",
]);
const PROJECT_PRIORITY = new Set<ProjectBuilderPriority>([
  "urgent",
  "high",
  "medium",
  "low",
  "none",
]);
const CALENDAR_DATE = /^\d{4}-\d{2}-\d{2}$/;

export function parseProjectBuilderDraft(content: string): ProjectBuilderDraftPayload | null {
  const matches = [...content.matchAll(PROJECT_DRAFT_TAG)];
  const raw = matches.at(-1)?.[1];
  if (!raw) return null;
  try {
    const value = JSON.parse(raw);
    return value && typeof value === "object"
      ? (value as ProjectBuilderDraftPayload)
      : null;
  } catch {
    // A CLI-backed model may put a literal line break in a JSON string even
    // after being instructed to emit one-line JSON. Repair only control
    // characters inside strings; the JSON structure must still parse.
    try {
      const value = JSON.parse(escapeJsonStringControlCharacters(raw));
      return value && typeof value === "object"
        ? (value as ProjectBuilderDraftPayload)
        : null;
    } catch {
      return null;
    }
  }
}

function escapeJsonStringControlCharacters(value: string): string {
  let result = "";
  let inString = false;
  let escaped = false;
  for (const character of value) {
    if (!inString) {
      result += character;
      if (character === '"') inString = true;
      continue;
    }
    if (escaped) {
      result += character;
      escaped = false;
      continue;
    }
    if (character === "\\") {
      result += character;
      escaped = true;
      continue;
    }
    if (character === '"') {
      result += character;
      inString = false;
      continue;
    }
    if (character === "\n") result += "\\n";
    else if (character === "\r") result += "\\r";
    else if (character === "\t") result += "\\t";
    else result += character;
  }
  return result;
}

export function stripProjectBuilderDraft(content: string): string {
  return content
    .replace(/\s*<project_draft>[\s\S]*?<\/project_draft>/g, "")
    .replace(/\s*<project_draft>[\s\S]*$/, "")
    .trim();
}

export function decodeProjectBuilderInput(content: string): string {
  if (!content.startsWith(PROJECT_BUILDER_INPUT_PREFIX)) return content;
  try {
    const parsed = JSON.parse(content.slice(PROJECT_BUILDER_INPUT_PREFIX.length)) as {
      user_request?: unknown;
    };
    return typeof parsed.user_request === "string"
      ? parsed.user_request
      : content;
  } catch {
    return content;
  }
}

export function encodeProjectBuilderInput(
  request: string,
  draft: ProjectBuilderDraft,
  catalogs: ProjectBuilderCatalogs,
): string {
  return (
    PROJECT_BUILDER_INPUT_PREFIX +
    JSON.stringify(
      {
        user_request: request,
        project_builder_instructions: PROJECT_BUILDER_RESPONSE_INSTRUCTIONS,
        current_draft: draft,
        available_workspace_members: catalogs.members,
        available_workspace_agents: catalogs.agents,
        available_project_labels: catalogs.labels,
        available_projects: catalogs.projects,
      },
      null,
      2,
    )
  );
}

function stringArray(value: unknown): string[] | null {
  if (!Array.isArray(value)) return null;
  return value.filter((item): item is string => typeof item === "string");
}

function validDate(value: unknown): string | null | undefined {
  if (value === null) return null;
  return typeof value === "string" && CALENDAR_DATE.test(value.trim())
    ? value.trim()
    : undefined;
}

/**
 * Applies one parsed assistant proposal while treating every catalog ID as
 * untrusted input. Existing values are preserved when the assistant omits a
 * field or proposes an ID that was not present in the catalogs supplied for
 * this turn.
 */
export function mergeProjectBuilderDraft(
  current: ProjectBuilderDraft,
  payload: ProjectBuilderDraftPayload,
  catalogs: ProjectBuilderCatalogs,
): ProjectBuilderDraft {
  const memberIDs = new Set(catalogs.members.map((item) => item.id));
  const agentIDs = new Set(catalogs.agents.map((item) => item.id));
  const labelIDs = new Set(catalogs.labels.map((item) => item.id));
  const projectIDs = new Set(catalogs.projects.map((item) => item.id));
  const next: ProjectBuilderDraft = { ...current };

  if (typeof payload.title === "string") next.title = payload.title;
  if (typeof payload.summary === "string") next.summary = payload.summary;
  if (typeof payload.description === "string") next.description = payload.description;
  if (typeof payload.icon === "string" || payload.icon === null) next.icon = payload.icon;
  if (typeof payload.status === "string" && PROJECT_STATUS.has(payload.status as ProjectBuilderStatus)) {
    next.status = payload.status as ProjectBuilderStatus;
  }
  if (
    typeof payload.priority === "string" &&
    PROJECT_PRIORITY.has(payload.priority as ProjectBuilderPriority)
  ) {
    next.priority = payload.priority as ProjectBuilderPriority;
  }

  const memberIDsFromPayload = stringArray(payload.member_ids);
  if (memberIDsFromPayload) next.member_ids = memberIDsFromPayload.filter((id) => memberIDs.has(id));
  const labelIDsFromPayload = stringArray(payload.label_ids);
  if (labelIDsFromPayload) next.label_ids = labelIDsFromPayload.filter((id) => labelIDs.has(id));
  const dependencyIDsFromPayload = stringArray(payload.dependency_ids);
  if (dependencyIDsFromPayload) {
    next.dependency_ids = dependencyIDsFromPayload.filter((id) => projectIDs.has(id));
  }

  if (Object.prototype.hasOwnProperty.call(payload, "lead_type") ||
      Object.prototype.hasOwnProperty.call(payload, "lead_id")) {
    if (payload.lead_type === null || payload.lead_id === null) {
      next.lead_type = null;
      next.lead_id = null;
    } else if (
      (payload.lead_type === "member" || payload.lead_type === "agent") &&
      typeof payload.lead_id === "string" &&
      ((payload.lead_type === "member" && memberIDs.has(payload.lead_id)) ||
        (payload.lead_type === "agent" && agentIDs.has(payload.lead_id)))
    ) {
      next.lead_type = payload.lead_type;
      next.lead_id = payload.lead_id;
    }
  }

  const startDate = validDate(payload.start_date);
  if (startDate !== undefined || payload.start_date === null) next.start_date = startDate;
  const dueDate = validDate(payload.due_date);
  if (dueDate !== undefined || payload.due_date === null) next.due_date = dueDate;

  return next;
}
