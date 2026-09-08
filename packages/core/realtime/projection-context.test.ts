// @vitest-environment node
import { QueryClient, QueryObserver } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { WSClient } from "../api/ws-client";
import { chatKeys } from "../chat/queries";
import { dingtalkKeys } from "../dingtalk/queries";
import { inboxKeys } from "../inbox/queries";
import { issueStatusKeys } from "../issue-statuses/queries";
import { issueKeys } from "../issues/queries";
import { setCurrentWorkspace } from "../platform/workspace-storage";
import type { ChatMessage, ChatPendingTask, Issue, ListIssuesCache, TaskMessagePayload } from "../types";
import { invalidateWorkspaceScopedQueries, subscribeCatalogProjection } from "./catalog-projection";
import { subscribeChatProjection } from "./chat-projection";
import { subscribeIssueProjection } from "./issue-projection";
import { createProjectionContext } from "./projection-context";
import type { RealtimeSyncStores } from "./use-realtime-sync";

class Socket {
  static latest: Socket;
  onopen: (() => void) | null = null;
  onclose: (() => void) | null = null;
  onerror: (() => void) | null = null;
  onmessage: ((event: { data: string }) => void) | null = null;
  constructor() { Socket.latest = this; }
  send() { }
  close() { }
  emit(type: string, payload: unknown) { this.onmessage?.({ data: JSON.stringify({ type, payload }) }); }
}

const issue: Issue = {
  id: "issue-a", workspace_id: "a", number: 1, identifier: "A-1", title: "Before",
  description: null, status: "todo", priority: "none", owner_type: null, owner_id: null,
  executor_type: null, executor_id: null, reviewer_type: null, reviewer_id: null,
  creator_type: "member", creator_id: "u", parent_issue_id: null, project_id: null,
  position: 0, stage: null, start_date: null, due_date: null, metadata: {}, properties: {},
  created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z", revision: 1,
};

const authStore = { getState: () => ({ user: { id: "u" } }) } as RealtimeSyncStores["authStore"];

// Canonical boundary -> domain -> real Query cache checks. React mounting and
// UI notifications remain covered by the existing use-realtime-sync suites.
describe("scoped realtime projections", () => {
  let qc: QueryClient;
  let cleanup: (() => void)[];

  beforeEach(() => {
    vi.useFakeTimers();
    vi.stubGlobal("WebSocket", Socket);
    setCurrentWorkspace("a", "a");
    cleanup = [];
    qc = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity, gcTime: Infinity } } });
  });
  afterEach(() => {
    cleanup.forEach((dispose) => dispose());
    qc.clear();
    setCurrentWorkspace(null, null);
    vi.unstubAllGlobals();
    vi.useRealTimers();
  });

  function connect(workspaceId = "a") {
    const ws = new WSClient("ws://example.test/ws");
    ws.setAuth("token", workspaceId);
    ws.connect();
    const scope = createProjectionContext(ws, {
      qc, workspaceId, workspaceSlug: workspaceId, authStore,
      hasOnboarded: () => true, onChatSessionDeleted: vi.fn(),
    });
    const unsubscribe = [subscribeCatalogProjection(scope), subscribeIssueProjection(scope), subscribeChatProjection(scope)];
    cleanup.push(() => { scope.dispose(); unsubscribe.forEach((fn) => fn()); ws.disconnect(); });
    return { ws, scope, socket: Socket.latest };
  }

  it("rejects malformed known frames before cache writes and continues with a valid issue", () => {
    const { socket } = connect();
    qc.setQueryData(issueKeys.detail("a", issue.id), issue);
    socket.emit("issue:updated", { issue: { ...issue, title: { invalid: true }, revision: 2 } });
    socket.emit("issue:updated", { issue: null });
    expect(qc.getQueryData(issueKeys.detail("a", issue.id))).toEqual(issue);
    socket.emit("issue:updated", { issue: { ...issue, title: "After", revision: 2 } });
    expect(qc.getQueryData(issueKeys.detail("a", issue.id))).toMatchObject({ title: "After", revision: 2 });
  });

  it("keeps issue revision ordering and task sequence deduplication through the parser", () => {
    const { socket } = connect();
    qc.setQueryData(issueKeys.detail("a", issue.id), issue);
    socket.emit("issue:updated", { issue: { ...issue, title: "Newest", revision: 3 } });
    socket.emit("issue:updated", { issue: { ...issue, title: "Older", revision: 2 } });
    expect(qc.getQueryData(issueKeys.detail("a", issue.id))).toMatchObject({ title: "Newest", revision: 3 });
    qc.setQueryData(chatKeys.taskMessages("t"), []);
    for (const seq of [3, 1, 3, 2]) socket.emit("task:message", { task_id: "t", type: "text", seq, content: String(seq) });
    expect(qc.getQueryData(chatKeys.taskMessages("t"))).toEqual([]);
    vi.advanceTimersByTime(100);
    expect(qc.getQueryData<TaskMessagePayload[]>(chatKeys.taskMessages("t"))?.map((row) => row.seq)).toEqual([1, 2, 3]);
  });

  it("rejects malformed nested labels before an issue snapshot can poison the cache", () => {
    const { socket } = connect();
    qc.setQueryData(issueKeys.detail("a", issue.id), issue);
    socket.emit("issue:updated", { issue: { ...issue, labels: [null], revision: 2 } });
    expect(qc.getQueryData(issueKeys.detail("a", issue.id))).toEqual(issue);
    const label = { id: "label-1", workspace_id: "a", name: "Ready", color: "#123456", created_at: "2026-01-01", updated_at: "2026-01-01" };
    socket.emit("issue:updated", { issue: { ...issue, labels: [label], revision: 2 } });
    const cached = qc.getQueryData<Issue>(issueKeys.detail("a", issue.id));
    expect(cached?.labels?.map((row) => row.name).join(", ")).toBe("Ready");
  });

  it("refreshes DingTalk installations for the current binding producer event", () => {
    const { socket } = connect();
    const key = dingtalkKeys.installations("a");
    qc.setQueryData(key, { installations: [] });
    // Matches RedeemDingTalkBindingToken's published payload.
    socket.emit("dingtalk_installation:binding_updated", { id: "installation-1" });
    vi.advanceTimersByTime(100);
    expect(qc.getQueryState(key)?.isInvalidated).toBe(true);
  });

  it("refreshes catalogs within the coalescing window during a sustained event stream", () => {
    const { socket } = connect();
    const invalidate = vi.spyOn(qc, "invalidateQueries");
    for (let index = 0; index < 6; index += 1) {
      socket.emit("issue_status:changed", { action: "updated" });
      vi.advanceTimersByTime(40);
    }
    expect(invalidate.mock.calls.filter(([options]) =>
      JSON.stringify(options?.queryKey) === JSON.stringify(issueStatusKeys.all("a"))
    )).toHaveLength(2);
  });

  it.each(["owner", "executor"] as const)("diffs an omitted %s change flag but respects explicit false", (dimension) => {
    const { socket } = connect();
    const key = issueKeys.myListSorted("a", "all", {}, undefined);
    const cached = { ...issue, [`${dimension}_type`]: dimension === "owner" ? "member" : "agent", [`${dimension}_id`]: "before" };
    const updated = { ...cached, [`${dimension}_id`]: "after", revision: 2 };
    const data = { byStatus: { todo: { issues: [cached], total: 1 } } };
    qc.setQueryData<ListIssuesCache>(key, data);
    qc.setQueryData(issueKeys.detail("a", issue.id), cached);
    socket.emit("issue:updated", { issue: updated });
    expect(qc.getQueryState(key)?.isInvalidated).toBe(true);
    qc.setQueryData<ListIssuesCache>(key, data);
    qc.setQueryData(issueKeys.detail("a", issue.id), cached);
    socket.emit("issue:updated", { issue: updated, [`${dimension}_changed`]: false });
    expect(qc.getQueryState(key)?.isInvalidated).toBe(false);
  });

  it("schedules a fresh catalog refresh when a previous generation still has a timer", () => {
    const { ws, socket } = connect();
    const key = issueStatusKeys.all("a");
    qc.setQueryData(key, []);
    socket.emit("issue_status:changed", { action: "updated" });
    ws.connect();
    Socket.latest.emit("issue_status:changed", { action: "updated" });
    vi.advanceTimersByTime(100);
    expect(qc.getQueryState(key)?.isInvalidated).toBe(true);
  });

  it("preserves an async projection listener's rejection for transport isolation", async () => {
    const { socket, scope } = connect();
    const failure = new Error("private rejected content");
    let handled = false;
    // Returning a thenable lets us assert that the transport observes the
    // rejection without deliberately leaking an unhandled rejection in RED.
    scope.ws.on("issue:deleted", () => ({
      then: (_resolve: unknown, reject: (reason: unknown) => void) => { handled = true; reject(failure); },
    }));
    const later = vi.fn();
    scope.ws.on("issue:deleted", later);
    socket.emit("issue:deleted", { issue_id: "another-issue" });
    await Promise.resolve();
    await Promise.resolve();
    expect(handled).toBe(true);
    expect(later).toHaveBeenCalledOnce();
  });

  it("preserves newer session state and refetches for an unknown session status", () => {
    const { socket } = connect();
    const session = { id: "s", title: "Newest", status: "active", updated_at: "2026-01-02T00:00:00Z" };
    qc.setQueryData(chatKeys.sessions("a"), [session]);
    socket.emit("chat:session_updated", { chat_session_id: "s", title: "Older", updated_at: "2026-01-01T00:00:00Z" });
    expect(qc.getQueryData(chatKeys.sessions("a"))).toEqual([session]);
    socket.emit("chat:session_updated", { chat_session_id: "s", status: "future_status", updated_at: "2026-01-03T00:00:00Z" });
    expect(qc.getQueryData(chatKeys.sessions("a"))).toEqual([session]);
    expect(qc.getQueryState(chatKeys.sessions("a"))?.isInvalidated).toBe(true);
  });

  it("does not resurrect a completed task on duplicate terminals or late running hints", () => {
    const { socket } = connect();
    qc.setQueryData<ChatPendingTask>(chatKeys.pendingTask("s"), { task_id: "t", status: "running" });
    socket.emit("task:completed", { task_id: "t", chat_session_id: "s" });
    socket.emit("task:completed", { task_id: "t", chat_session_id: "s" });
    socket.emit("task:running", { task_id: "t", chat_session_id: "s" });
    expect(qc.getQueryData<ChatPendingTask>(chatKeys.pendingTask("s"))?.task_id).toBeUndefined();
  });

  it("drops A callbacks and queued batches immediately when the route switches to B", () => {
    const a = connect();
    const invalidate = vi.spyOn(qc, "invalidateQueries");
    qc.setQueryData(chatKeys.taskMessages("task-a"), []);
    a.socket.emit("task:message", { task_id: "task-a", seq: 1, type: "text" });
    a.socket.emit("task:completed", { task_id: "task-a", chat_session_id: "session-a" });
    a.socket.emit("issue_status:changed", { action: "created" });
    invalidate.mockClear();
    setCurrentWorkspace("b", "b");
    // The old connection still exists during React's cleanup/effect gap.
    a.socket.emit("chat:message", { chat_session_id: "session-a", message_id: "m", role: "user", content: "A prompt", created_at: "2026-01-01" });
    a.socket.emit("issue:updated", { issue: { ...issue, title: "Late A" } });
    vi.advanceTimersByTime(1000);
    expect(qc.getQueryData(chatKeys.taskMessages("task-a"))).toEqual([]);
    expect(qc.getQueryData(chatKeys.messages("session-a"))).toBeUndefined();
    expect(qc.getQueryData(issueKeys.detail("b", issue.id))).toBeUndefined();
    expect(invalidate).not.toHaveBeenCalled();
    const b = connect("b");
    b.socket.emit("issue_status:changed", { action: "created" });
    vi.advanceTimersByTime(100);
    expect(invalidate).toHaveBeenCalledWith({ queryKey: issueStatusKeys.all("b") });
  });

  it("cannot bind an A socket to B even if subscription setup runs after the route changed", () => {
    const ws = new WSClient("ws://example.test/ws");
    ws.setAuth("token", "a");
    ws.connect();
    setCurrentWorkspace("b", "b");
    const scope = createProjectionContext(ws, { qc, workspaceId: "b", workspaceSlug: "b", authStore, hasOnboarded: () => true, onChatSessionDeleted: vi.fn() });
    const handler = vi.fn();
    const unsubscribe = scope.ws.onAny(handler);
    Socket.latest.emit("issue:deleted", { issue_id: "issue-a" });
    expect(handler).not.toHaveBeenCalled();
    scope.dispose(); unsubscribe(); ws.disconnect();
  });

  it("abandons a quick-actions write suspended across connection generations", async () => {
    const { ws, socket } = connect();
    const message: ChatMessage = { id: "m", chat_session_id: "s", task_id: "t", role: "assistant", content: "Hello", created_at: "2026-01-01", quick_actions: [] };
    qc.setQueryData(chatKeys.messages("s"), [message]);
    let release!: () => void;
    const pending = new Promise<void>((resolve) => { release = resolve; });
    vi.spyOn(qc, "cancelQueries").mockReturnValue(pending);
    socket.emit("chat:quick_actions", { chat_session_id: "s", task_id: "t", message_id: "m", quick_actions: [{ label: "Next", prompt: "Continue" }] });
    // Reconnect keeps the same WSClient and projection subscriptions.
    ws.connect();
    release();
    await pending;
    await Promise.resolve();
    expect(qc.getQueryData(chatKeys.messages("s"))).toEqual([message]);
  });

  it("reconciles an active query after reconnect and discards the previous connection's batch", async () => {
    const { ws, socket, scope } = connect();
    let serverRows: TaskMessagePayload[] = [];
    const fetchRows = vi.fn(async () => serverRows);
    const observer = new QueryObserver(qc, { queryKey: chatKeys.taskMessages("t"), queryFn: fetchRows, staleTime: Infinity });
    const unsubscribe = observer.subscribe(() => { });
    cleanup.push(unsubscribe);
    await observer.refetch();
    ws.onReconnect(() => { if (scope.isActive()) invalidateWorkspaceScopedQueries(qc, "a"); });
    socket.emit("auth_ack", undefined);
    socket.emit("task:message", { task_id: "t", seq: 1, type: "text", content: "pre-disconnect" });
    socket.onclose?.();
    serverRows = [{ task_id: "t", issue_id: "", seq: 1, type: "text", content: "authoritative" }];
    await vi.advanceTimersByTimeAsync(1500);
    Socket.latest.emit("auth_ack", undefined);
    await vi.advanceTimersByTimeAsync(1);
    expect(qc.getQueryData(chatKeys.taskMessages("t"))).toEqual(serverRows);
    expect(fetchRows).toHaveBeenCalledTimes(2);
  });

  it("keeps personal task aggregates server-owned when another member broadcasts a task", () => {
    const { socket } = connect();
    qc.setQueryData(chatKeys.pendingTasks("a"), []);
    qc.setQueryData(chatKeys.pendingTasksHasAny("a"), { has_pending: false });
    qc.setQueryData(inboxKeys.all("a"), []);
    const write = vi.spyOn(qc, "setQueryData");
    socket.emit("task:running", { task_id: "someone-elses-task", chat_session_id: "private-session" });
    vi.advanceTimersByTime(750);
    expect(qc.getQueryData(chatKeys.pendingTasks("a"))).toEqual([]);
    expect(qc.getQueryData(chatKeys.pendingTasksHasAny("a"))).toEqual({ has_pending: false });
    expect(qc.getQueryState(chatKeys.pendingTasks("a"))?.isInvalidated).toBe(true);
    expect(write.mock.calls.some(([key]) => JSON.stringify(key) === JSON.stringify(chatKeys.pendingTasks("a")))).toBe(false);
  });
});
