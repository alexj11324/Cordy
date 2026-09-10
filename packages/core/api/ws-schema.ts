import { z } from "zod";
import type { WSMessage } from "../types/events";
import {
  ChatMessageSchema,
  MessageCitationSchema,
  MessageSourceSchema,
  CommentSchema,
  InboxItemListSchema,
  IssuePropertyValuesSchema,
  IssueSchema,
  LabelSchema,
  TimelineEntriesSchema,
  WorkspaceChannelSchema,
  WorkspaceChannelMessageSchema,
} from "./schemas";

const id = z.string().min(1);
const revision = z.number().int().positive().optional();
const object = z.object({}).loose();
const issueReaction = z.object({
  id, issue_id: id, actor_type: z.string(), actor_id: id,
  emoji: z.string(), created_at: z.string(), issue_revision: revision,
}).loose();
const wsIssue = IssueSchema.extend({
  id, workspace_id: id,
  // Full snapshots feed both label renderers and mobile's reaction row.
  // HTTP's loose arrays must not admit null entries into realtime caches.
  labels: z.array(LabelSchema.extend({ id })).optional(),
  reactions: z.array(issueReaction).optional(),
});
const issueId = z.object({ issue_id: id, issue_revision: revision }).loose();
const sessionId = z.object({ chat_session_id: id }).loose();
const task = z.object({
  task_id: id,
  // Chat tasks omit issue_id in protocol/messages.go. Older lifecycle
  // producers also omit status/agent_id; no projection relies on them.
  issue_id: z.string().optional(),
  chat_session_id: id.optional(),
  agent_id: z.string().optional(),
  status: z.string().optional(),
  runtime_id: z.string().optional(),
  wait_reason: z.string().optional(),
  failure_reason: z.string().optional(),
  retry_pending: z.boolean().optional(),
}).loose();
const comment = z.object({
  comment: CommentSchema.extend({
    id, issue_id: id,
    // TaskService.commentEventFields and runtime_unusable_notice publish
    // sparse comment snapshots; missing display dates are not missing identity.
    parent_id: z.string().nullable().default(null),
    created_at: z.string().default(""), updated_at: z.string().default(""),
    resolved_at: z.string().nullable().optional(),
    resolved_by_type: z.string().nullable().optional(),
    resolved_by_id: z.string().nullable().optional(),
  }),
  issue_revision: revision,
}).loose();
const quickActions = z.array(z.object({
  label: z.string(), prompt: z.string(), primary: z.boolean().optional(),
}).loose()).nullish().transform((value) => value ?? undefined);
const issueProperties = z.record(z.string(), z.unknown()).transform((values, ctx) => {
  const parsed = IssuePropertyValuesSchema.safeParse(values);
  if (!parsed.success) {
    for (const issue of parsed.error.issues) ctx.addIssue({ ...issue });
    return z.NEVER;
  }
  return parsed.data;
});
const chatDoneFields = {
  chat_session_id: id,
  task_id: id,
  message_id: id.optional(),
  content: z.string().optional(),
  sources: z.array(MessageSourceSchema).optional(),
  citations: z.array(MessageCitationSchema).optional(),
  created_at: z.string().optional(),
  elapsed_ms: z.number().nonnegative().optional(),
  message_kind: ChatMessageSchema.shape.message_kind,
};

/**
 * Guard the fields consumers read or merge, retaining unknown additive fields
 * and string enums. These shapes follow protocol/messages.go, handler issue /
 * comment responses and cmd/server/listeners.go's personal-event routing.
 * An invalid known frame is dropped, never replaced by an empty domain object.
 */
const payloadSchemas: Record<string, z.ZodType> = {
  "issue:created": z.object({ issue: wsIssue }).loose(),
  "issue:updated": z.object({
    issue: wsIssue,
    owner_changed: z.boolean().optional(),
    executor_changed: z.boolean().optional(),
    reviewer_changed: z.boolean().optional(),
    review_handoff: z.boolean().optional(),
    status_changed: z.boolean().optional(),
    project_changed: z.boolean().optional(),
  }).loose(),
  "issue:deleted": issueId,
  "issue_attachments:changed": issueId,
  "issue_labels:changed": issueId.extend({ labels: z.array(LabelSchema.extend({ id })) }),
  "issue_metadata:changed": issueId.extend({ metadata: z.record(z.string(), z.union([z.string(), z.number(), z.boolean()])) }),
  "issue_properties:changed": issueId.extend({ properties: issueProperties }),
  "comment:created": comment,
  "comment:updated": comment,
  "comment:resolved": comment,
  "comment:unresolved": comment,
  "comment:deleted": issueId.extend({ comment_id: id }),
  "activity:created": issueId.extend({ entry: TimelineEntriesSchema.element.extend({ id }) }),
  "reaction:added": issueId.extend({
    reaction: z.object({ id, comment_id: id, actor_type: z.string(), actor_id: id, emoji: z.string(), created_at: z.string() }).loose(),
    comment_revision: revision,
  }),
  "reaction:removed": issueId.extend({
    comment_id: id, actor_type: z.string(), actor_id: id, emoji: z.string(), comment_revision: revision,
  }),
  "issue_reaction:added": issueId.extend({
    reaction: issueReaction,
  }),
  "issue_reaction:removed": issueId.extend({ actor_type: z.string(), actor_id: id, emoji: z.string() }),
  "subscriber:added": issueId.extend({ user_type: z.string(), user_id: id, reason: z.string() }),
  "subscriber:removed": issueId.extend({ user_type: z.string(), user_id: id }),
  "inbox:new": z.object({ item: InboxItemListSchema.element.extend({ id, workspace_id: id }) }).loose(),
  "workspace:updated": z.object({
    workspace: z.object({
      id, name: z.string(), slug: id, issue_prefix: z.string(),
      description: z.string().nullable(), context: z.string().nullable(),
      settings: z.record(z.string(), z.unknown()),
      repos: z.array(z.object({ url: z.string(), description: z.string().optional() }).loose()),
      lead_agent_id: z.string().nullable().default(null),
      avatar_url: z.string().nullable(), created_at: z.string(), updated_at: z.string(),
    }).loose()
  }).loose(),
  "workspace:deleted": z.object({ workspace_id: id }).loose(),
  "member:added": z.object({ workspace_id: id.optional(), member: z.object({ user_id: id }).loose(), workspace_name: z.string().optional() }).loose(),
  "member:removed": z.object({ workspace_id: id, user_id: id, member_id: id }).loose(),
  "invitation:created": z.object({ invitation: object, workspace_name: z.string().optional() }).loose(),
  "task:queued": task,
  "task:dispatch": task,
  "task:running": task,
  "task:waiting_local_directory": task,
  "task:completed": task,
  "task:failed": task,
  "task:cancelled": task,
  "task:message": task.extend({
    seq: z.number().int().nonnegative(), type: id,
    call_id: z.string().optional(), state: z.string().optional(),
    tool: z.string().optional(), content: z.string().optional(),
    sources: z.array(MessageSourceSchema).optional(),
    citations: z.array(MessageCitationSchema).optional(),
    input: z.record(z.string(), z.unknown()).optional(),
    output: z.string().optional(), created_at: z.string().optional(),
  }),
  "chat:message": sessionId.extend({ message_id: id, role: z.string(), content: z.string(), sources: z.array(MessageSourceSchema).optional(), citations: z.array(MessageCitationSchema).optional(), task_id: id.optional(), created_at: z.string() }),
  "chat:done": z.object({ ...chatDoneFields, quick_actions: quickActions, quick_actions_pending: z.boolean().optional() }).loose(),
  "chat:quick_actions": sessionId.extend({ task_id: id, message_id: id, quick_actions: quickActions, failed: z.boolean().optional() }),
  "chat:cancel_finalized": z.object({ ...chatDoneFields, outcome: z.string(), initiator_user_id: id.optional() }).loose(),
  "chat:session_read": sessionId,
  "chat:session_deleted": sessionId,
  "chat:session_created": sessionId.extend({ workspace_id: id }),
  "chat:session_updated": sessionId.extend({
    title: z.string().optional(), project_id: id.nullable().optional(),
    pinned: z.boolean().optional(), status: z.string().optional(), updated_at: z.string().optional(),
  }),
  "channel:created": z.object({
    channel: WorkspaceChannelSchema.extend({
      id, workspace_id: id, name: id, slug: id, description: z.string(),
      created_by: id, created_at: id, updated_at: id,
    }),
  }).loose(),
  "channel:message": z.object({
    channel_id: id,
    message: WorkspaceChannelMessageSchema.extend({
      id, workspace_id: id, channel_id: id, author_type: id, author_id: id,
      content: z.string().min(1), created_at: id, updated_at: id,
    }),
  }).loose(),
  "dependency_graph:updated": z.object({
    plan_id: id.nullish(), plan_ids: z.array(id).optional(),
    parent_issue_id: id.optional(), promoted_issue_ids: z.array(id).optional(),
  }).loose(),
  "dingtalk_installation:binding_updated": z.object({ id }).loose(),
};

// These consumers only invalidate authoritative queries; their payload is an
// opaque object. Include the integration prefixes already consumed by the
// catalog projection even where the historical WSEventType union omitted them.
for (const event of [
  "agent:status", "agent:created", "agent:archived", "agent:restored",
  "inbox:read", "inbox:unread", "inbox:archived", "inbox:unarchived", "inbox:batch-read", "inbox:batch-archived",
  "member:updated", "daemon:heartbeat", "daemon:register", "task:progress",
  "skill:created", "skill:updated", "skill:deleted",
  "project:created", "project:updated", "project:deleted",
  "team:created", "team:updated", "team:deleted", "label:created", "label:updated", "label:deleted",
  "property:created", "property:updated", "issue_status:changed", "issue_category_policy:changed",
  "pin:created", "pin:deleted", "pin:reordered",
  "invitation:accepted", "invitation:declined", "invitation:revoked",
  "github_installation:created", "github_installation:deleted", "dingtalk_group_route:updated",
  "pull_request:linked", "pull_request:updated", "pull_request:unlinked",
  "automation:created", "automation:updated", "automation:deleted", "automation:run_start", "automation:run_done",
  "lark_installation:created", "lark_installation:revoked", "slack_installation:created", "slack_installation:revoked",
  "dingtalk_installation:created", "dingtalk_installation:revoked", "vcs_connection:created", "vcs_connection:deleted",
  "wecom_installation:created", "wecom_installation:revoked", "weixin_installation:created", "weixin_installation:revoked",
  "telegram_installation:created", "telegram_installation:revoked",
]) payloadSchemas[event] = object;

const envelope = z.object({
  type: id, payload: z.unknown().optional(), actor_id: z.string().optional(), actor_type: z.string().optional(),
}).loose();

type InvalidFrame = { kind: "invalid"; issues: { path: string; code: string }[] };
export type ParsedWSFrame = InvalidFrame | { kind: "unknown" } | { kind: "auth" } | { kind: "event"; message: WSMessage };

function invalid(error: z.ZodError): InvalidFrame {
  return { kind: "invalid", issues: error.issues.map((issue) => ({ path: issue.path.join("."), code: issue.code })) };
}

export function parseWSFrame(data: unknown): ParsedWSFrame {
  const parsed = envelope.safeParse(data);
  if (!parsed.success) return invalid(parsed.error);
  const message = parsed.data;
  if (message.type === "auth_ack") return { kind: "auth" };
  if (!Object.hasOwn(payloadSchemas, message.type)) return { kind: "unknown" };
  const payload = payloadSchemas[message.type]!.safeParse(message.payload);
  if (!payload.success) return invalid(payload.error);
  return { kind: "event", message: { ...message, payload: payload.data } as WSMessage };
}
