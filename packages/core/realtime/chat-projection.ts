"use client";

import { type InfiniteData, type QueryClient } from "@tanstack/react-query";
import { upsertChatMessageToCaches } from "../chat/message-cache";
import {
  promotePendingChatTask,
  removePendingChatTask,
} from "../chat/pending";
import {
  chatKeys,
  isTaskMessageTimelineHeld,
  mergeTaskMessagesBySeq,
  QUICK_ACTIONS_PENDING_TIMEOUT_MS,
  sortChatSessions,
} from "../chat/queries";
import { createLogger } from "../logger";
import type {
  ChatCancelFinalizedPayload,
  ChatDonePayload,
  ChatMessage,
  ChatMessageEventPayload,
  ChatMessagesPage,
  ChatPendingTask,
  ChatQuickActionsFailureState,
  ChatQuickActionsPayload,
  ChatQuickActionsPendingState,
  ChatSession,
  ChatSessionCreatedPayload,
  TaskCancelledPayload,
  TaskCompletedPayload,
  TaskDispatchPayload,
  TaskFailedPayload,
  TaskMessagePayload,
  TaskQueuedPayload,
  TaskRunningPayload,
  TaskWaitingLocalDirectoryPayload
} from "../types";

import type { ProjectionContext } from "./projection-context";
const chatWsLogger = createLogger("chat.ws");
const TASK_MESSAGE_FLUSH_MS = 100;

export function invalidateChatMessageQueries(
  qc: QueryClient,
  sessionId: string,
) {
  qc.invalidateQueries({ queryKey: chatKeys.messages(sessionId) });
  qc.invalidateQueries({ queryKey: chatKeys.messagesPage(sessionId) });
}

// refetchPendingChatAggregate marks the current user's cross-session pending
// aggregate stale so it is refetched from the permission-filtering endpoint
// (/api/chat/pending-tasks[/has-any]).
//

// invalidate, NOT an optimistic setQueryData. Chat `task:*` events are a
// workspace fanout delivered to every member with no creator / agent
// visibility in the payload, so optimistically writing the aggregate from them
// would let one member's task flip another member's FAB to has_pending=true,
// bypassing the server-side permission filter. Invalidation forces the
// authoritative, creator+agent-scoped server response to be the source of
// truth. The has-any key is nested under pendingTasks, so invalidating
// pendingTasks refreshes both the detailed list and the boolean fast-path.
export function refetchPendingChatAggregate(
  qc: QueryClient,
  wsId: string | null | undefined,
) {
  if (!wsId) return;
  qc.invalidateQueries({ queryKey: chatKeys.pendingTasks(wsId) });
}


export function applyChatMessageToCache(
  qc: QueryClient,
  payload: ChatMessageEventPayload,
) {
  const sessionId = payload.chat_session_id;
  if (payload.role === "user" && payload.message_id) {
    upsertChatMessageToCaches(qc, sessionId, {
      id: payload.message_id,
      chat_session_id: sessionId,
      role: "user",
      content: payload.content ?? "",
      ...(payload.sources !== undefined ? { sources: payload.sources } : {}),
      ...(payload.citations !== undefined ? { citations: payload.citations } : {}),
      task_id: payload.task_id ?? null,
      created_at: payload.created_at ?? new Date().toISOString(),
    });
  }
  invalidateChatMessageQueries(qc, sessionId);
  qc.invalidateQueries({ queryKey: chatKeys.pendingTask(sessionId) });
}

export function applyChatDoneToCache(
  qc: QueryClient,
  payload: ChatDonePayload,
) {
  const sessionId = payload.chat_session_id;
  const taskId = payload.task_id;
  const messageId = payload.message_id;
  const content = payload.content;
  if (messageId && (content !== undefined || (payload.quick_actions?.length ?? 0) > 0)) {
    const assistant: ChatMessage = {
      id: messageId,
      chat_session_id: sessionId,
      role: "assistant",
      content: content ?? "",
      ...(payload.sources !== undefined ? { sources: payload.sources } : {}),
      ...(payload.citations !== undefined ? { citations: payload.citations } : {}),
      task_id: taskId,
      created_at: payload.created_at ?? new Date().toISOString(),
      elapsed_ms: payload.elapsed_ms ?? null,
      // Carry the kind so a no_response turn renders its placeholder inline

      // "message" for older servers.
      message_kind: payload.message_kind ?? "message",
      ...(payload.quick_actions !== undefined
        ? { quick_actions: payload.quick_actions }
        : {}),
    };
    // Idempotent against reconnect replay and against a refetch that already
    // landed this row.
    upsertChatMessageToCaches(qc, sessionId, assistant);
  }
  // Replacement is in the messages list now; remove only this task. If a
  // follow-up is queued, it becomes the next head in the same render tick.
  qc.setQueryData<ChatPendingTask>(
    chatKeys.pendingTask(sessionId),
    (old) => removePendingChatTask(old, taskId),
  );
  // Raise/clear the quick-actions placeholder marker. Kept OUTSIDE the
  // message caches deliberately: the authoritative refetch below replaces
  // those, and a flag stored on the message would vanish with it. Explicit
  // `=== true` — older servers omit the field entirely.
  qc.setQueryData<ChatQuickActionsPendingState | null>(
    chatKeys.quickActionsPending(sessionId),
    payload.quick_actions_pending === true && messageId
      ? {
        message_id: messageId,
        task_id: taskId,
        expires_at: Date.now() + QUICK_ACTIONS_PENDING_TIMEOUT_MS,
      }
      : null,
  );
  // Authoritative refetch reconciles redaction / migrations / clients
  // that took the fallback branch above.
  invalidateChatMessageQueries(qc, sessionId);
  qc.invalidateQueries({ queryKey: chatKeys.pendingTask(sessionId) });
}


export async function applyChatQuickActionsToCache(
  qc: QueryClient,
  payload: ChatQuickActionsPayload,
  isCurrent: () => boolean = () => true,
) {
  const sessionId = payload.chat_session_id;
  const actions = payload.quick_actions ?? [];
  const patch = (m: ChatMessage): ChatMessage =>
    m.id === payload.message_id ? { ...m, quick_actions: actions } : m;
  if (actions.length > 0) {
    // chat:done's invalidate may still have a messages refetch in flight that
    // read the assistant row BEFORE the daemon persisted these actions. Cancel
    // it first so its actions-less response can't land after — and overwrite —
    // the patch below. Both message caches are staleTime: Infinity, so such an

    // before setQueryData is required: cancelQueries reverts to the pre-fetch
    // state, so patching first would be undone by the revert.
    await Promise.all([
      qc.cancelQueries({ queryKey: chatKeys.messages(sessionId) }),
      qc.cancelQueries({ queryKey: chatKeys.messagesPage(sessionId) }),
    ]);
    if (!isCurrent()) return;
    qc.setQueryData<ChatMessage[] | undefined>(
      chatKeys.messages(sessionId),
      (old) => old?.map(patch),
    );
    qc.setQueryData<InfiniteData<ChatMessagesPage> | undefined>(
      chatKeys.messagesPage(sessionId),
      (old) =>
        old
          ? {
            ...old,
            pages: old.pages.map((page) => ({
              ...page,
              messages: page.messages.map(patch),
            })),
          }
          : old,
    );

    // so the line above does more than ignore the in-flight response — it rolls
    // the cache back to the snapshot taken when that fetch STARTED, dropping
    // rows only that response carried (a peer's user message, anything that
    // landed while this surface was unmounted). With staleTime: Infinity and no
    // further trigger, the hole survived until a remount. Re-invalidating costs
    // one request per supplement and cannot lose the pills: the server persists
    // the actions BEFORE broadcasting this event (SupplementChatQuickActions),
    // so the refetch this schedules reads them back.
    invalidateChatMessageQueries(qc, sessionId);
  }
  // Resolve the marker only when it belongs to THIS message: a late
  // supplement for turn N must not clear the marker turn N+1's chat:done
  // just raised (near-unreachable — the daemon cancels stale passes — but
  // the guard costs one comparison).
  qc.setQueryData<ChatQuickActionsPendingState | null>(
    chatKeys.quickActionsPending(sessionId),
    (current) =>
      current && current.message_id !== payload.message_id ? current : null,
  );
  // Raise a one-shot failure signal on an explicit refresh whose regeneration
  // failed. A view consumes it to toast and clears it; the `at` nonce keeps a
  // repeat failure on the same turn from being deduped away.
  if (payload.failed === true) {
    qc.setQueryData<ChatQuickActionsFailureState | null>(
      chatKeys.quickActionsFailure(sessionId),
      { message_id: payload.message_id, at: Date.now() },
    );
  }
}

type ChatSessionUpdatedPayload = {
  chat_session_id: string;
  title?: string;
  project_id?: string | null;
  pinned?: boolean;
  status?: "active" | "archived";
  updated_at?: string;
};


export function applyChatSessionUpdatedToCache(
  qc: QueryClient,
  wsId: string,
  payload: ChatSessionUpdatedPayload,
): void {
  if (payload.status !== undefined && payload.status !== "active" && payload.status !== "archived") {
    qc.invalidateQueries({ queryKey: chatKeys.sessions(wsId) });
    return;
  }
  qc.setQueryData<ChatSession[]>(chatKeys.sessions(wsId), (old) => {
    if (!old) return old;
    const next = old.map((s) =>
      s.id === payload.chat_session_id
        && !(payload.updated_at && s.updated_at && Date.parse(payload.updated_at) < Date.parse(s.updated_at))
        ? {
          ...s,
          title: payload.title ?? s.title,
          ...("project_id" in payload ? { project_id: payload.project_id } : {}),
          pinned: payload.pinned ?? s.pinned,
          status: payload.status ?? s.status,
          updated_at: payload.updated_at ?? s.updated_at,
          ...(payload.status === "archived"
            ? { unread_count: 0, has_unread: false }
            : {}),
        }
        : s,
    );
    return payload.pinned === undefined && payload.status === undefined
      ? next
      : sortChatSessions(next);
  });
}

function removeChatMessageFromPageCache(
  qc: QueryClient,
  sessionId: string,
  messageId: string,
) {
  qc.setQueryData<InfiniteData<ChatMessagesPage> | undefined>(
    chatKeys.messagesPage(sessionId),
    (old) => {
      if (!old) return old;
      return {
        ...old,
        pages: old.pages.map((page) => ({
          ...page,
          messages: page.messages.filter((m) => m.id !== messageId),
        })),
      };
    },
  );
}

export function removeChatMessageFromCaches(
  qc: QueryClient,
  sessionId: string,
  messageId: string,
) {
  qc.setQueryData<ChatMessage[]>(
    chatKeys.messages(sessionId),
    (old) => old?.filter((m) => m.id !== messageId) ?? old,
  );
  removeChatMessageFromPageCache(qc, sessionId, messageId);
}

/**
 * Apply a chat:cancel_finalized event (#5219): the deferred outcome of a
 * cancelled chat task, settled after the daemon's transcript flush.
 *
 * - outcome "stopped": a late "Stopped." assistant row was persisted —
 *   insert it exactly like a chat:done message.
 * - outcome "restored": the triggering user message was deleted — drop it
 *   from the caches. The deleted prompt itself never rides this
 *   workspace-wide broadcast: it is durable server-side and only the
 *   initiator's client refetches it through the creator-authorized
 *   draft-restores query, which the session's composer applies and consumes.
 *
 * The draft-restores invalidation is gated to the task's initiator and fails
 * closed when initiator_user_id is missing — nothing is lost either way,
 * because the durable restore is fetched again on the next composer mount or
 * network reconnect. Cache patches stay unconditional — they are no-ops for
 * anyone not viewing the session.
 */
export function applyChatCancelFinalizedToCache(
  qc: QueryClient,
  payload: ChatCancelFinalizedPayload,
  currentUserId?: string,
) {
  const sessionId = payload.chat_session_id;
  if (!sessionId) return;
  if (payload.outcome === "stopped") {
    applyChatDoneToCache(qc, {
      chat_session_id: sessionId,
      task_id: payload.task_id,
      message_id: payload.message_id,
      content: payload.content,
      elapsed_ms: payload.elapsed_ms,
      created_at: payload.created_at,
      message_kind: payload.message_kind,
    });
    return;
  }
  if (payload.outcome === "restored") {
    if (payload.message_id) {
      removeChatMessageFromCaches(qc, sessionId, payload.message_id);
    }
    // Deferred finalization can arrive after a queued successor was promoted.
    // Remove only the cancelled task so a late restore cannot hide newer work.
    qc.setQueryData<ChatPendingTask>(
      chatKeys.pendingTask(sessionId),
      (old) => removePendingChatTask(old, payload.task_id),
    );
    invalidateChatMessageQueries(qc, sessionId);
    qc.invalidateQueries({ queryKey: chatKeys.pendingTask(sessionId) });
    const isInitiator =
      !!payload.initiator_user_id &&
      !!currentUserId &&
      payload.initiator_user_id === currentUserId;
    if (isInitiator) {
      void qc.invalidateQueries({ queryKey: chatKeys.draftRestores(sessionId) });
    }
  }
}

export function subscribeChatProjection({ ws, qc, workspaceId, authStore, isActive, captureGuard, onChatSessionDeleted }: ProjectionContext) {
  // --- Chat / task events (global, survives ChatWindow unmount) ---
  //
  // Single source of truth: the Query cache. No Zustand writes here — the
  // earlier mirror caused a race where the cache and store disagreed
  // during the invalidate → refetch window and the UI rendered duplicates.
  //
  // task:message is written directly into the task-messages cache so the
  // live timeline updates in place. chat:message / chat:done /
  // task:completed / task:failed invalidate messages + pending-task so the
  // DB remains authoritative.

  // Two guards stand between the workspace-wide message firehose and the

  // EVERY run in the workspace, but only the handful of runs a user actually
  // opens is ever rendered:
  //
  // 1. Frames are kept only for a task this client already holds a timeline
  //    entry for — opened at some point, and not yet garbage-collected. The
  //    old `(old = [])` default built that entry on first sight instead, so
  //    every client accumulated the transcript of every run its user would
  //    never open, unbounded tool input included.
  // 2. Frames that survive that gate are coalesced into one cache write per
  //    window, so a burst costs one merge and one render instead of N.
  //
  // The entry can be collected between the two, which is why the flush
  // re-checks rather than trusting the gate — see flushTaskMessages.
  const taskMessageBatches = new Map<string, TaskMessagePayload[]>();
  let taskMessageFlushTimer: ReturnType<typeof setTimeout> | null = null;

  let batchIsCurrent = isActive;
  const flushTaskMessages = () => {
    if (!batchIsCurrent()) {
      taskMessageBatches.clear();
      taskMessageFlushTimer = null;
      return;
    }
    taskMessageFlushTimer = null;

    for (const [taskId, batch] of taskMessageBatches) {
      // Re-check, because holding was last verified up to a window ago and
      // `setQueryData` does NOT postpone garbage collection — query-core arms
      // that timer when the last observer leaves and never again on write.
      // Closing a transcript while its run keeps streaming therefore has the
      // entry disappear mid-window, and writing then REBUILDS it holding only
      // this batch. With the app-wide `staleTime: Infinity` the next open
      // would read that stub as fresh and never fetch, so everything before
      // it would be missing until the window is reloaded. Dropping the batch
      // instead costs nothing: the rows are persisted, so the next open
      // fetches the whole timeline.
      if (!isTaskMessageTimelineHeld(qc, taskId)) {
        continue;
      }
      qc.setQueryData<TaskMessagePayload[]>(
        chatKeys.taskMessages(taskId),
        (old = []) => mergeTaskMessagesBySeq(old, batch),
      );
    }
    taskMessageBatches.clear();
  };

  const unsubTaskMessage = ws.on("task:message", (p) => {
    const payload = p as TaskMessagePayload;
    // Cheap Map lookup, and it runs before anything allocates — this is the
    // hot path for every run in the workspace, not just the visible ones.
    if (!isTaskMessageTimelineHeld(qc, payload.task_id)) return;

    if (!batchIsCurrent()) taskMessageBatches.clear();
    batchIsCurrent = captureGuard();
    const batch = taskMessageBatches.get(payload.task_id);
    if (batch) batch.push(payload);
    else taskMessageBatches.set(payload.task_id, [payload]);

    // Fixed window, not a resetting debounce: a continuous stream must still
    // flush every TASK_MESSAGE_FLUSH_MS instead of being starved until a gap.
    if (!taskMessageFlushTimer) {
      taskMessageFlushTimer = setTimeout(flushTaskMessages, TASK_MESSAGE_FLUSH_MS);
    }

    chatWsLogger.debug("task:message (global)", {
      task_id: payload.task_id,
      seq: payload.seq,
      type: payload.type,
    });
  });

  // Helpers reused by chat lifecycle handlers.
  //

  // *workspace fanout* — every member of the workspace receives them — and
  // the payload carries no creator / agent-visibility. So we must NEVER
  // optimistically write the cross-session pending AGGREGATE
  // (chatKeys.pendingTasks / chatKeys.pendingTasksHasAny) from these events:
  // member B starting a chat task would otherwise flip member A's FAB to
  // has_pending=true, bypassing the server-side permission filter on
  // /api/chat/pending-tasks[/has-any]. Instead we authoritatively (debounced)
  // invalidate the aggregate so it is refetched through the filtering
  // endpoint, which only returns the caller's own creator-owned,
  // accessible-agent tasks.
  //
  // The per-session `pendingTask` cache IS still written directly by the
  // handlers below — it is keyed by chat_session_id and only rendered for a
  // session the user is allowed to open (server-gated), so it is not a
  // cross-user aggregate leak.
  //
  // chat:message is intentionally NOT a trigger (it fires per streamed

  // aggregate is refreshed only on task lifecycle transitions, which are
  // per-task and low-frequency, then coalesced by the debounce below.
  let aggregateRefreshTimer: ReturnType<typeof setTimeout> | null = null;
  const invalidatePendingAggregate = () => {
    if (aggregateRefreshTimer) clearTimeout(aggregateRefreshTimer);
    const isCurrent = captureGuard();
    aggregateRefreshTimer = setTimeout(() => {
      if (!isCurrent()) return;
      aggregateRefreshTimer = null;
      refetchPendingChatAggregate(qc, workspaceId);
    }, 750);
  };
  const invalidateSessionLists = () => {
    const id = workspaceId;
    if (id) qc.invalidateQueries({ queryKey: chatKeys.sessions(id) });
  };

  const unsubChatMessage = ws.on("chat:message", (p) => {
    const payload = p as ChatMessageEventPayload;
    chatWsLogger.info("chat:message (global)", {
      chat_session_id: payload.chat_session_id,
      role: payload.role,
    });
    // Write the user turn before invalidating so the prompt does not depend

    applyChatMessageToCache(qc, payload);
    // NOTE: intentionally does NOT touch the pending aggregate. chat:message
    // fires per streamed message with no status; the aggregate is maintained

  });

  const unsubChatDone = ws.on("chat:done", (p) => {
    const payload = p as ChatDonePayload;
    chatWsLogger.info("chat:done (global)", {
      task_id: payload.task_id,
      chat_session_id: payload.chat_session_id,
      has_message: !!payload.message_id,
    });
    // Inline-insert the assistant message into the messages cache BEFORE
    // clearing pending-task. Both writes land in the same React render
    // tick, so ChatMessageList sees `pendingAlreadyPersisted === true`
    // and the live TimelineView unmounts only after AssistantMessage has
    // mounted — no flicker window. This applies TkDodo's "combine
    // setQueryData (active query) + invalidateQueries (others)" pattern
    // (https://tkdodo.eu/blog/using-web-sockets-with-react-query).
    //
    // Falls back to invalidate-only when the server omits the message
    // payload (older builds). Older clients hitting a newer server also
    // work: they ignore the extra fields and rely on the invalidate
    // below, which keeps the old behavior alive.
    applyChatDoneToCache(qc, payload);
    // NOTE: the pending aggregate is left to the task:completed / task:failed
    // handlers (which carry the task_id needed to remove the right entry).
    // chat:done no longer invalidates it, so a chatty session doesn't refetch

    // Assistant message just landed → has_unread may have flipped to true.
    invalidateSessionLists();
  });

  // Late quick-actions supplement from the daemon's background suggestion
  // pass — patches the finished turn's message in place; no invalidate
  // needed (the payload is authoritative and tiny).
  const unsubChatQuickActions = ws.on("chat:quick_actions", (p) => {
    const payload = p as ChatQuickActionsPayload;
    chatWsLogger.info("chat:quick_actions (global)", {
      task_id: payload.task_id,
      chat_session_id: payload.chat_session_id,
      count: payload.quick_actions?.length ?? 0,
    });
    void applyChatQuickActionsToCache(qc, payload, captureGuard());
  });

  // Deferred cancellation outcome (#5219): the server settles the
  // empty/non-empty judgment only after the daemon's transcript flush, so
  // this event arrives seconds after the cancel HTTP response — nothing
  // else re-fetches at that point.
  const unsubChatCancelFinalized = ws.on("chat:cancel_finalized", (p) => {
    const payload = p as ChatCancelFinalizedPayload;
    chatWsLogger.info("chat:cancel_finalized (global)", {
      task_id: payload.task_id,
      chat_session_id: payload.chat_session_id,
      outcome: payload.outcome,
    });
    applyChatCancelFinalizedToCache(qc, payload, authStore.getState().user?.id);
    if (payload.outcome === "stopped") {
      // A Stopped. assistant row just landed → session previews change.
      invalidateSessionLists();
    }
  });

  // Lifecycle events are invalidation hints. They intentionally omit queue
  // previews and message ids, so only the pending-task endpoint can author
  // the complete queue shape.
  const unsubTaskQueued = ws.on("task:queued", (p) => {
    const payload = p as TaskQueuedPayload;
    if (!payload.chat_session_id) return;
    qc.invalidateQueries({ queryKey: chatKeys.pendingTask(payload.chat_session_id) });
    invalidatePendingAggregate();
  });

  // task:dispatch fires when the daemon claims the queued task. The daemon
  // immediately follows with StartTask, so dispatched→running is sub-second.
  // We collapse that window by writing "running" directly — the pill jumps
  // from "Queued" straight to "Thinking", skipping a meaningless "Starting"
  // frame. Stage decision in TaskStatusPill maps "running" + empty
  // taskMessages → "Thinking · Ns".
  const unsubTaskDispatch = ws.on("task:dispatch", (p) => {
    const payload = p as TaskDispatchPayload;
    if (!payload.chat_session_id) return;
    qc.setQueryData<ChatPendingTask>(
      chatKeys.pendingTask(payload.chat_session_id),
      (old) => promotePendingChatTask(old, payload.task_id, "running"),
    );
    invalidateChatMessageQueries(qc, payload.chat_session_id);
    qc.invalidateQueries({ queryKey: chatKeys.pendingTask(payload.chat_session_id) });
    invalidatePendingAggregate();
  });

  // task:running fires when the daemon transitions a previously-parked task
  // (waiting_local_directory) back into the run phase. The dispatch→running
  // path is collapsed in the handler above, so this handler exists mainly to
  // clear a stale `waiting_local_directory` pill — without it, the pill
  // would stay parked even after the daemon resumed work.
  const unsubTaskRunning = ws.on("task:running", (p) => {
    const payload = p as TaskRunningPayload;
    if (!payload.chat_session_id) return;
    qc.setQueryData<ChatPendingTask>(
      chatKeys.pendingTask(payload.chat_session_id),
      (old) => promotePendingChatTask(old, payload.task_id, "running"),
    );
    invalidateChatMessageQueries(qc, payload.chat_session_id);
    qc.invalidateQueries({ queryKey: chatKeys.pendingTask(payload.chat_session_id) });
    invalidatePendingAggregate();
  });

  // task:waiting_local_directory fires when the daemon dequeues a task but
  // can't acquire the local_directory path lock — another task on this
  // daemon is in the same directory. Write the status so TaskStatusPill
  // can render the "Waiting for local directory" stage instead of pinning
  // a stale "Starting / Thinking" frame.
  const unsubTaskWaitingLocalDir = ws.on(
    "task:waiting_local_directory",
    (p) => {
      const payload = p as TaskWaitingLocalDirectoryPayload;
      if (!payload.chat_session_id) return;
      qc.setQueryData<ChatPendingTask>(
        chatKeys.pendingTask(payload.chat_session_id),
        (old) =>
          promotePendingChatTask(
            old,
            payload.task_id,
            "waiting_local_directory",
            undefined,
            payload.wait_reason,
          ),
      );
      invalidateChatMessageQueries(qc, payload.chat_session_id);
      qc.invalidateQueries({ queryKey: chatKeys.pendingTask(payload.chat_session_id) });
      invalidatePendingAggregate();
    },
  );

  // task:cancelled reaches us when:
  //   1. handleStop already cleared the cache locally (this is a no-op confirm)
  //   2. another tab / admin / system cancels — this is the only path that
  //      drops the pending pill in those cases. Without it the pill spins
  //      forever in the second-tab scenario.
  // CancelTask also persists a best-effort assistant snapshot when the
  // stopped chat task had already streamed transcript rows, so refresh the
  // message page along with clearing pending.
  const unsubTaskCancelled = ws.on("task:cancelled", (p) => {
    const payload = p as TaskCancelledPayload;
    if (!payload.chat_session_id) return;
    chatWsLogger.info("task:cancelled (global, chat)", {
      task_id: payload.task_id,
      chat_session_id: payload.chat_session_id,
    });
    qc.setQueryData<ChatPendingTask>(
      chatKeys.pendingTask(payload.chat_session_id),
      (old) => removePendingChatTask(old, payload.task_id),
    );
    qc.invalidateQueries({ queryKey: chatKeys.pendingTask(payload.chat_session_id) });
    invalidateChatMessageQueries(qc, payload.chat_session_id);
    invalidatePendingAggregate();
    invalidateSessionLists();
  });

  const unsubTaskCompleted = ws.on("task:completed", (p) => {
    const payload = p as TaskCompletedPayload;
    if (!payload.chat_session_id) return; // issue tasks handled elsewhere
    chatWsLogger.info("task:completed (global, chat)", {
      task_id: payload.task_id,
      chat_session_id: payload.chat_session_id,
    });
    qc.setQueryData<ChatPendingTask>(
      chatKeys.pendingTask(payload.chat_session_id),
      (old) => removePendingChatTask(old, payload.task_id),
    );
    qc.invalidateQueries({ queryKey: chatKeys.pendingTask(payload.chat_session_id) });
    invalidatePendingAggregate();
  });

  const unsubTaskFailed = ws.on("task:failed", (p) => {
    const payload = p as TaskFailedPayload;
    if (!payload.chat_session_id) return;
    chatWsLogger.warn("task:failed (global, chat)", {
      task_id: payload.task_id,
      chat_session_id: payload.chat_session_id,
    });
    // FailTask writes a failure chat_message (mirroring CompleteTask's
    // success message), so this path mirrors the task:completed handler:
    // clear the pending signal AND invalidate the messages list so the
    // failure bubble shows up without requiring a page refresh. Pre-#1823
    // this branch only flipped pending — the comment "No new message"
    // was true then, but FailTask now persists a row.
    qc.setQueryData<ChatPendingTask>(
      chatKeys.pendingTask(payload.chat_session_id),
      (old) => removePendingChatTask(old, payload.task_id),
    );
    invalidateChatMessageQueries(qc, payload.chat_session_id);
    qc.invalidateQueries({ queryKey: chatKeys.pendingTask(payload.chat_session_id) });
    invalidatePendingAggregate();
    // FailTask persisted a failure chat_message, so the thread list's
    // last_message / unread_count / sort order changed too. Mirror the
    // chat:done (success) path and refresh the sessions list — otherwise the
    // left rail keeps a stale preview until the next full refetch.
    invalidateSessionLists();
  });

  const unsubChatSessionRead = ws.on("chat:session_read", (p) => {
    const payload = p as { chat_session_id: string };
    chatWsLogger.info("chat:session_read (global)", payload);
    invalidateSessionLists();
  });

  const unsubChatSessionCreated = ws.on("chat:session_created", (p) => {
    const payload = p as ChatSessionCreatedPayload;
    chatWsLogger.info("chat:session_created (global)", payload);
    if (payload.workspace_id !== workspaceId) return;
    invalidateSessionLists();
  });

  // chat:session_updated fires after the creator renames, pins, or archives
  // a session in any tab/device. Patch the cached row inline so the dropdown
  // and badges reflect the change without a full sessions-list refetch — see
  // applyChatSessionUpdatedToCache for why archive must also zero unread.
  const unsubChatSessionUpdated = ws.on("chat:session_updated", (p) => {
    const payload = p as ChatSessionUpdatedPayload;
    chatWsLogger.info("chat:session_updated (global)", payload);
    const id = workspaceId;
    if (!id) return;
    applyChatSessionUpdatedToCache(qc, id, payload);
  });

  // chat:session_deleted fires after a hard delete. The originating tab has
  // already optimistically dropped the row via useDeleteChatSession; this
  // handler keeps OTHER tabs/devices in sync and also clears the active
  // session pointer so a deleted session doesn't keep the chat window
  // pointed at vanished messages.
  const unsubChatSessionDeleted = ws.on("chat:session_deleted", (p) => {
    const payload = p as { chat_session_id: string };
    chatWsLogger.info("chat:session_deleted (global)", payload);
    const id = workspaceId;
    if (id) {
      const drop = (old?: { id: string }[]) =>
        old?.filter((s) => s.id !== payload.chat_session_id);
      qc.setQueryData(chatKeys.sessions(id), drop);
    }
    qc.removeQueries({ queryKey: chatKeys.messages(payload.chat_session_id) });
    qc.removeQueries({ queryKey: chatKeys.pendingTask(payload.chat_session_id) });
    invalidatePendingAggregate();

    onChatSessionDeleted(payload.chat_session_id);
  });

  return () => {
    unsubTaskMessage();
    unsubChatMessage();
    unsubChatDone();
    unsubChatQuickActions();
    unsubChatCancelFinalized();
    unsubTaskQueued();
    unsubTaskDispatch();
    unsubTaskRunning();
    unsubTaskWaitingLocalDir();
    unsubTaskCancelled();
    unsubTaskCompleted();
    unsubTaskFailed();
    unsubChatSessionRead();
    unsubChatSessionCreated();
    unsubChatSessionDeleted();
    unsubChatSessionUpdated();
    if (taskMessageFlushTimer) clearTimeout(taskMessageFlushTimer);
    if (aggregateRefreshTimer) clearTimeout(aggregateRefreshTimer);
  };
}
