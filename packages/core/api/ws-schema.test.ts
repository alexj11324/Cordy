// @vitest-environment node
import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { parseWSFrame } from "./ws-schema";

const issue = {
  id: "issue-1", workspace_id: "workspace-1", number: 1,
  identifier: "WS-1", title: "A real issue", description: null,
  status: "in_progress", priority: "medium", creator_type: "member",
  creator_id: "member-1", parent_issue_id: null, project_id: null,
  position: 1, start_date: null, due_date: null, metadata: {},
  created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z",
};

const frame = (type: string, payload: unknown) => parseWSFrame({ type, payload });

// workspace_channel.go publishes the generated database rows in these wrappers.
const channel = {
  id: "channel-1", workspace_id: "workspace-1", name: "Updates", slug: "updates",
  description: "", created_by: "user-1", archived_at: null,
  created_at: "2026-09-02T00:00:00Z", updated_at: "2026-09-02T00:00:00Z",
};
const channelMessage = {
  id: "message-1", workspace_id: "workspace-1", channel_id: "channel-1",
  author_type: "member", author_id: "user-1", content: "Ready for review",
  parent_id: null, quoted_message_id: null,
  created_at: "2026-09-02T00:01:00Z", updated_at: "2026-09-02T00:01:00Z",
};

describe("WebSocket inbound contract", () => {
  it.each([null, [], 42, "frame", {}, { type: 12 }, { type: "" }, { type: "issue:deleted", actor_id: {} }])("rejects malformed envelope %j", (data) => {
    expect(parseWSFrame(data).kind).toBe("invalid");
  });

  it.each([
    ["issue:updated", null],
    ["issue:created", { issue: { ...issue, id: "" } }],
    ["issue:updated", { issue: { ...issue, workspace_id: 3 } }],
    ["issue:created", { issue: { ...issue, labels: [null] } }],
    ["issue:updated", { issue: { ...issue, labels: [{ name: 42 }] } }],
    ["issue:updated", { issue: { ...issue, reactions: [null] } }],
    ["dingtalk_installation:binding_updated", { id: null }],
    ["issue:updated", { issue, owner_changed: "false" }],
    ["issue:deleted", { issue_id: [] }],
    ["issue_labels:changed", { issue_id: "issue-1", labels: "all" }],
    ["issue_properties:changed", { issue_id: "issue-1", properties: [] }],
    ["comment:created", { comment: null }],
    ["task:message", { task_id: "task-1", seq: "2", type: "text" }],
    ["task:message", { task_id: "task-1", seq: 1.5, type: "text" }],
    ["task:message", { task_id: "task-1", seq: -1, type: "text" }],
    ["task:message", { task_id: "task-1", seq: 1, type: "text", content: {} }],
    ["task:completed", { task_id: "", chat_session_id: "session-1" }],
    ["chat:done", { task_id: "task-1", chat_session_id: null }],
    ["chat:quick_actions", { task_id: "task-1", chat_session_id: "s", message_id: "m", quick_actions: [{ label: "Do it", prompt: {} }] }],
    ["chat:session_updated", { chat_session_id: "s", pinned: "false" }],
    ["member:removed", { workspace_id: "ws", user_id: "user" }],
    ["inbox:new", { item: { id: "i", workspace_id: [] } }],
    ["channel:created", { channel: null }],
    ["channel:created", { channel: { ...channel, id: "" } }],
    ["channel:created", { channel: { ...channel, workspace_id: 42 } }],
    ["channel:message", { channel_id: "channel-1", message: null }],
    ["channel:message", { channel_id: "channel-1", message: { ...channelMessage, channel_id: "" } }],
    ["channel:message", { channel_id: "channel-1", message: { ...channelMessage, content: {} } }],
  ])("drops malformed known %s payload", (type, payload) => {
    expect(frame(type, payload).kind).toBe("invalid");
  });

  it("preserves new issue fields and new enum values while accepting old optional omissions", () => {
    const parsed = frame("issue:updated", { issue: { ...issue, status: "custom_review", priority: "future", future_field: { enabled: true } } });
    expect(parsed.kind).toBe("event");
    if (parsed.kind !== "event") return;
    expect(parsed.message.payload).toMatchObject({
      issue: {
        status: "custom_review", priority: "future", future_field: { enabled: true }, owner_id: null, properties: {},
      }
    });
  });

  it("accepts older chat:done and chat task messages without an issue id", () => {
    expect(frame("chat:done", { task_id: "t", chat_session_id: "s" }).kind).toBe("event");
    expect(frame("task:message", { task_id: "t", seq: 0, type: "future_stream_kind" }).kind).toBe("event");
  });

  it("validates nested labels while retaining optional and additive producer fields", () => {
    const label = {
      id: "label-1", workspace_id: "workspace-1", name: "Ready", color: "#123456",
      created_at: "2026-01-01", updated_at: "2026-01-01", future_field: true,
    };
    for (const type of ["issue:created", "issue:updated"]) {
      expect(frame(type, { issue: { ...issue, labels: [label] } })).toMatchObject({
        kind: "event", message: { payload: { issue: { labels: [{ ...label, resource_type: "issue" }] } } },
      });
      expect(frame(type, { issue }).kind).toBe("event");
    }
  });

  it("accepts the DingTalk binding producer's installation id payload", () => {
    // server/internal/handler/dingtalk.go: RedeemDingTalkBindingToken.
    expect(frame("dingtalk_installation:binding_updated", { id: "installation-1" })).toMatchObject({
      kind: "event", message: { payload: { id: "installation-1" } },
    });
  });

  it("accepts the current channel producer wrappers and preserves additive fields", () => {
    expect(frame("channel:created", { channel: { ...channel, future_field: true } })).toMatchObject({
      kind: "event", message: { payload: { channel: { ...channel, future_field: true } } },
    });
    expect(frame("channel:message", { channel_id: channel.id, message: channelMessage })).toMatchObject({
      kind: "event", message: { payload: { channel_id: channel.id, message: channelMessage } },
    });
  });

  it("accepts invitation and share-link member additions without a redundant workspace id", () => {
    const member = { id: "member-1", user_id: "user-1", workspace_id: "workspace-1", role: "member" };
    for (const payload of [{ member }, { member, workspace_name: "Our workspace" }]) {
      expect(frame("member:added", payload)).toMatchObject({ kind: "event", message: { payload } });
    }
    expect(frame("member:added", { member: { user_id: null } }).kind).toBe("invalid");
  });

  it("preserves omitted versus explicitly false issue change flags", () => {
    const omitted = frame("issue:updated", { issue });
    const explicit = frame("issue:updated", { issue, owner_changed: false, executor_changed: false });
    expect(omitted).toMatchObject({ kind: "event", message: { payload: { issue } } });
    if (omitted.kind !== "event") return;
    expect(omitted.message.payload).not.toHaveProperty("owner_changed");
    expect(omitted.message.payload).not.toHaveProperty("executor_changed");
    expect(explicit).toMatchObject({ kind: "event", message: { payload: { owner_changed: false, executor_changed: false } } });
  });

  it("accepts current sparse system and agent comment producers", () => {
    const comment = { id: "c", issue_id: "i", author_type: "system", author_id: "system", content: "Runtime unavailable", type: "system" };
    expect(frame("comment:created", { comment })).toMatchObject({ kind: "event", message: { payload: { comment: { id: "c", created_at: "" } } } });
    expect(frame("comment:created", { comment: { ...comment, parent_id: null, created_at: "2026-01-01T00:00:00Z" } }).kind).toBe("event");
  });

  it("normalizes unknown display kinds and a nil quick-action slice safely", () => {
    const done = frame("chat:done", { task_id: "t", chat_session_id: "s", message_kind: "future_kind" });
    expect(done).toMatchObject({ kind: "event", message: { payload: { message_kind: "message" } } });
    const supplement = frame("chat:quick_actions", { task_id: "t", chat_session_id: "s", message_id: "m", quick_actions: null });
    expect(supplement).toMatchObject({ kind: "event", message: { payload: { quick_actions: undefined } } });
  });

  it("ignores unknown events even when their prefix or name resembles a known key", () => {
    for (const type of ["future:event", "task:future_transition", "channel:future_event", "constructor", "toString"]) {
      expect(frame(type, { private: "ignored" }).kind).toBe("unknown");
    }
  });

  it("keeps existing invalidation-only integration events and future advisory enum values", () => {
    for (const type of ["lark_installation:revoked", "slack_installation:revoked", "telegram_installation:revoked", "automation:run_done", "issue_status:changed"]) {
      expect(frame(type, { action: "future_action" }).kind).toBe("event");
    }
  });

  it("recognizes server-declared events for every consumed catalog prefix", () => {
    const protocol = readFileSync(new URL("../../../server/pkg/protocol/events.go", import.meta.url), "utf8");
    const catalog = readFileSync(new URL("../realtime/catalog-projection.ts", import.meta.url), "utf8");
    const refreshMap = catalog.split("const refreshMap:")[1]!.split("const timers =")[0]!;
    const prefixes = new Set([...refreshMap.matchAll(/^ {4}(\w+): \(\) =>/gm)].map((match) => match[1]));
    expect(prefixes.has("dingtalk_installation")).toBe(true);
    // These are daemon control-channel messages, never client catalog
    // broadcasts. Heartbeat/register still belong to the client registry.
    const daemonControl = new Set([
      "daemon:heartbeat_ack", "daemon:task_available", "daemon:runtime_profiles_changed",
      "daemon:workspaces_changed", "daemon:pending_work", "daemon:rpc_request", "daemon:rpc_response",
    ]);
    const events = [...protocol.matchAll(/^\s*Event\w+\s*=\s*"([^"]+)"/gm)].map((match) => match[1]!);
    expect(events.length).toBeGreaterThan(50);
    const expected = events.filter((event) => prefixes.has(event.split(":")[0]) && !daemonControl.has(event));
    expect(expected).toContain("dingtalk_installation:binding_updated");
    expect(expected.filter((event) => frame(event, {}).kind === "unknown")).toEqual([]);
  });

  it("reports invalid field paths without copying rejected content", () => {
    const result = frame("chat:done", { task_id: "t", chat_session_id: { private_prompt: "secret" } });
    expect(result).toMatchObject({ kind: "invalid", issues: [{ path: "chat_session_id" }] });
    expect(JSON.stringify(result)).not.toContain("secret");
  });

  it("returns an invalid frame instead of throwing for JSON numeric overflow", () => {
    const overflowing = JSON.parse('{"type":"issue_properties:changed","payload":{"issue_id":"i","properties":{"p":1e400}}}');
    expect(() => parseWSFrame(overflowing)).not.toThrow();
    expect(parseWSFrame(overflowing)).toMatchObject({ kind: "invalid" });
  });

  it("recognizes channel producer events declared outside the protocol constant catalog", () => {
    const producer = readFileSync(new URL("../../../server/internal/handler/workspace_channel.go", import.meta.url), "utf8");
    const events = [...producer.matchAll(/^\s*workspaceChannel\w+Event\s*=\s*"([^"]+)"/gm)].map((match) => match[1]!);
    expect(events).toContain("channel:created");
    expect(events).toContain("channel:message");
    expect(events.filter((event) => frame(event, {}).kind === "unknown")).toEqual([]);
  });
});
