/**
 * Chat tab — single-screen IA.
 *
 * Layout:
 *   View ─ Header(center: ChatTitleButton, right: ChatSessionActions)
 *        ─ (NoAgentBanner?)
 *        ─ KeyboardAvoidingView ─ ChatMessageList (includes live status
 *                                                  + timeline in its
 *                                                  ListFooterComponent)
 *                                ─ OfflineBanner
 *                                ─ ChatComposer
 *
 * Session switching happens through the native `chat-sessions` formSheet;
 * agent selection and deletion stay in native controls — there is no
 * `/chat/[id]` sub-route.
 *
 * State (mobile-local and persisted per workspace):
 *   - activeSessionId   — which session is being viewed (null = new chat blank)
 *   - selectedAgentId   — overrides currentSession.agent_id when set (used
 *                         when starting a new chat with a freshly-picked agent)
 *   - sessionSheetOpen  — bottom modal visibility
 *   - agentPickerOpen   — bottom modal visibility
 *
 * Side effects:
 *   - useChatSessionRealtime(activeSessionId) for per-record WS events
 *   - auto markRead when entering a session with has_unread
 *   - ensureSession dedupe ref for concurrent first-message sends
 *
 * Optimistic send burst mirrors web's chat-window.tsx send sequence
 * (packages/views/chat/components/chat-window.tsx ~262-345):
 *   seed messages → seed pendingTask → flip activeSessionId → POST →
 *   patch pendingTask with server task_id + created_at.
 */
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  Alert,
  KeyboardAvoidingView,
  Platform,
  View,
} from "react-native";
import { router, useLocalSearchParams } from "expo-router";
import { useFocusEffect, useIsFocused } from "@react-navigation/native";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import type {
  Agent,
  ChatMessage,
  ChatPendingTask,
} from "@patchbay/core/types";
import {
  enqueuePendingChatTask,
  hideQueuedChatMessages,
  removePendingChatTask,
} from "@patchbay/core/chat/pending";
import { canAssignAgentToIssue } from "@patchbay/core/permissions";
import { api } from "@/data/api";
import { useAuthStore } from "@/data/auth-store";
import { useWorkspaceStore } from "@/data/workspace-store";
import { agentListOptions } from "@/data/queries/agents";
import { memberListOptions } from "@/data/queries/members";
import {
  chatKeys,
  chatMessagesOptions,
  chatSessionsOptions,
  pendingChatTaskOptions,
  taskMessagesOptions,
} from "@/data/queries/chat";
import {
  useCreateChatSession,
  useDeleteChatSession,
  useMarkChatSessionRead,
} from "@/data/mutations/chat";
import {
  DRAFT_NEW_SESSION,
  useChatDraftsStore,
} from "@/data/stores/chat-drafts-store";
import { useChatSessionPickerStore } from "@/data/stores/chat-session-picker-store";
import {
  loadChatActiveSession,
  saveChatActiveSession,
} from "@/data/chat-session-storage";
import { useChatSessionRealtime } from "@/data/realtime/use-chat-session-realtime";
import {
  invalidatePendingTask,
  seedAcceptedPendingTask,
} from "@/data/realtime/chat-ws-updaters";
import { useWorkspaceAgentAvailability } from "@/lib/workspace-agent-availability";
import { sendFailureMessage } from "@/lib/dispatch-reason";
import { useAgentPresence } from "@/lib/use-agent-presence";
import { Header } from "@/components/ui/header";
import { ChatTitleButton } from "@/components/chat/chat-title-button";
import { ChatSessionActions } from "@/components/chat/chat-session-actions";
import { ChatMessageList } from "@/components/chat/chat-message-list";
import { ChatComposer } from "@/components/chat/chat-composer";
import { AgentPickerSheet } from "@/components/chat/agent-picker-sheet";
import { NoAgentBanner } from "@/components/chat/no-agent-banner";
import { OfflineBanner } from "@/components/chat/offline-banner";
import { RuntimeRequiredBanner } from "@/components/chat/runtime-required-banner";
import { useChatSelectStore } from "@/data/chat-select-store";
import { isAgentRuntimeBound } from "@/lib/is-agent-runtime-bound";
import { chatSessionDisplayTitle } from "@/lib/chat-session-title";
import {
  chatRouteParams,
  firstChatRouteParam,
  resolveActiveChatSessionId,
} from "@/lib/chat-session-state";
import { useChatCopy } from "@/lib/use-chat-copy";

export default function ChatTab() {
  const qc = useQueryClient();
  const { agent: agentParam, session: sessionParam } = useLocalSearchParams<{
    agent?: string | string[];
    session?: string | string[];
  }>();
  const wsId = useWorkspaceStore((s) => s.currentWorkspaceId);
  const wsSlug = useWorkspaceStore((s) => s.currentWorkspaceSlug);
  const userId = useAuthStore((s) => s.user?.id);
  const copy = useChatCopy();

  const [activeSessionId, setActiveSessionId] = useState<string | null>(null);
  const [selectedAgentId, setSelectedAgentId] = useState<string | null>(null);
  const [agentPickerOpen, setAgentPickerOpen] = useState(false);
  const workspaceRef = useRef(wsId);
  const hydratedWsRef = useRef<string | null>(null);
  const restoredWsRef = useRef<string | null>(null);
  const routeIntentRef = useRef<string | null>(null);

  // Bridge to the chat-sessions formSheet route. Mirror local
  // activeSessionId into the store so the picker can render the current
  // selection's check mark; consume the picker's one-shot select request
  // via useEffect.
  const setStoreActiveSessionId = useChatSessionPickerStore(
    (s) => s.setActiveSessionId,
  );
  const selectRequest = useChatSessionPickerStore((s) => s.selectRequest);
  const consumeSelect = useChatSessionPickerStore((s) => s.consumeSelect);
  useEffect(() => {
    setStoreActiveSessionId(activeSessionId);
  }, [activeSessionId, setStoreActiveSessionId]);

  // ── Server state ───────────────────────────────────────────────────────
  const { data: sessions = [], isSuccess: sessionsLoaded } = useQuery(
    chatSessionsOptions(wsId),
  );
  const { data: agents = [], isSuccess: agentsLoaded } = useQuery(
    agentListOptions(wsId),
  );
  const { data: members = [], isSuccess: membersLoaded } = useQuery(
    memberListOptions(wsId),
  );

  const routeSessionId = firstChatRouteParam(sessionParam);
  const routeAgentId = firstChatRouteParam(agentParam);
  const routeIntentKey = `${wsId ?? ""}:${routeSessionId ?? ""}:${routeAgentId ?? ""}`;
  const { data: messages = [], isLoading: messagesLoading } = useQuery(
    chatMessagesOptions(activeSessionId),
  );
  const { data: pendingTask } = useQuery(
    pendingChatTaskOptions(activeSessionId),
  );
  const visibleMessages = hideQueuedChatMessages(messages, pendingTask);
  // Live execution trace for the in-flight task. `task:message` WS events
  // append rows to this same cache key via `appendTaskMessage`, so the
  // list/pill stay in sync without a polling fetch. `enabled` is gated by
  // `isTaskMessageTaskId` inside taskMessagesOptions — optimistic ids
  // never hit the network.
  const { data: liveTaskMessages = [] } = useQuery(
    taskMessagesOptions(pendingTask?.task_id),
  );

  // ── Derived ────────────────────────────────────────────────────────────
  const memberRole = useMemo(
    () => members.find((m) => m.user_id === userId)?.role ?? null,
    [members, userId],
  );

  // The picker must list only agents this user can actually TRIGGER — sending
  // a message enqueues a run, so it clears the server's invoke gate
  // (`canInvokeAgent`), which has no admin bypass. Shared rule, not a mobile
  // copy: a local mirror drifted from it and let admins pick a teammate's
  // personal agent only to be 403'd on send (MUL-6380 / GH #7180).
  const availableAgents = useMemo(
    () =>
      agents.filter(
        (a) =>
          !a.archived_at &&
          canAssignAgentToIssue(a, { userId: userId ?? null, role: memberRole })
            .allowed,
      ),
    [agents, userId, memberRole],
  );

  // ── Restore active session / deep-link intent ──────────────────────────
  // Mobile's native tab stays mounted across tab switches. Restore the
  // selected thread only after the sessions query is settled; otherwise the
  // initial `[]` during loading permanently suppresses hydration. A valid
  // `?session=` or permission-checked `?agent=` intent wins over the stored
  // selection, matching the web/desktop ChatPage entry contract.
  useEffect(() => {
    if (workspaceRef.current === wsId) return;
    workspaceRef.current = wsId;
    hydratedWsRef.current = null;
    restoredWsRef.current = null;
    routeIntentRef.current = null;
    setActiveSessionId(null);
    setSelectedAgentId(null);
  }, [wsId]);

  useEffect(() => {
    if (!wsId || !sessionsLoaded || !agentsLoaded || !membersLoaded) return;
    if (hydratedWsRef.current === wsId) return;
    hydratedWsRef.current = wsId;
    let cancelled = false;

    void loadChatActiveSession(wsId).then((persistedId) => {
      if (cancelled) return;
      // A new URL intent arrived while SecureStore was reading. Let the
      // route-intent effect own that newer selection instead of resurrecting
      // the old one after the user has navigated.
      if (
        routeIntentRef.current !== null &&
        routeIntentRef.current !== routeIntentKey
      ) {
        restoredWsRef.current = wsId;
        return;
      }

      const linkedSession = routeSessionId
        ? sessions.find((session) => session.id === routeSessionId)
        : null;
      const linkedAgent = routeAgentId
        ? availableAgents.find((agent) => agent.id === routeAgentId)
        : null;
      const nextSessionId = linkedSession
        ? linkedSession.id
        : linkedAgent
          ? null
          : resolveActiveChatSessionId(persistedId, sessions);

      setSelectedAgentId(linkedAgent && !linkedSession ? linkedAgent.id : null);
      setActiveSessionId(nextSessionId);
      restoredWsRef.current = wsId;
      routeIntentRef.current = routeIntentKey;
      void saveChatActiveSession(wsId, nextSessionId);
    });

    return () => {
      cancelled = true;
    };
  }, [
    agentsLoaded,
    availableAgents,
    membersLoaded,
    routeAgentId,
    routeIntentKey,
    routeSessionId,
    sessions,
    sessionsLoaded,
    wsId,
  ]);

  // Handle a new route intent while the tab remains mounted. The initial
  // restore above records the first intent, so changing only the query string
  // still opens the requested native thread/agent instead of being ignored.
  useEffect(() => {
    if (!wsId || !sessionsLoaded || !agentsLoaded || !membersLoaded) return;
    if (hydratedWsRef.current !== wsId) return;
    if (routeIntentRef.current === routeIntentKey) return;
    routeIntentRef.current = routeIntentKey;

    const linkedSession = routeSessionId
      ? sessions.find((session) => session.id === routeSessionId)
      : null;
    if (linkedSession) {
      setSelectedAgentId(null);
      setActiveSessionId(linkedSession.id);
      return;
    }

    const linkedAgent = routeAgentId
      ? availableAgents.find((agent) => agent.id === routeAgentId)
      : null;
    if (linkedAgent) {
      setSelectedAgentId(linkedAgent.id);
      setActiveSessionId(null);
    }
  }, [
    agentsLoaded,
    availableAgents,
    membersLoaded,
    routeAgentId,
    routeIntentKey,
    routeSessionId,
    sessions,
    sessionsLoaded,
    wsId,
  ]);

  useEffect(() => {
    if (!wsId || !sessionsLoaded || restoredWsRef.current !== wsId) return;
    void saveChatActiveSession(wsId, activeSessionId);
  }, [activeSessionId, sessionsLoaded, wsId]);

  // Keep the native tab deep-linkable after the user changes sessions or
  // starts a new agent thread. The restore gate prevents the initial render
  // from stripping a pending `?session=`/`?agent=` before SecureStore and the
  // settled agent/session queries have reconciled it. `router.setParams`
  // updates this tab in place, so the native sheet/back stack is untouched.
  useEffect(() => {
    if (!wsId || restoredWsRef.current !== wsId) return;
    const nextParams = chatRouteParams(activeSessionId, selectedAgentId);
    if (
      routeSessionId === (nextParams.session ?? null) &&
      routeAgentId === (nextParams.agent ?? null)
    ) {
      return;
    }
    router.setParams(nextParams);
  }, [
    activeSessionId,
    routeAgentId,
    routeSessionId,
    selectedAgentId,
    wsId,
  ]);

  const activeSession = useMemo(
    () => sessions.find((s) => s.id === activeSessionId) ?? null,
    [sessions, activeSessionId],
  );

  // Active agent: explicit selection wins; otherwise inherit from the
  // active session; otherwise pick the first available agent.
  const currentAgent: Agent | null = useMemo(() => {
    if (selectedAgentId) {
      return availableAgents.find((a) => a.id === selectedAgentId) ?? null;
    }
    if (activeSession) {
      return agents.find((a) => a.id === activeSession.agent_id) ?? null;
    }
    return availableAgents[0] ?? null;
  }, [selectedAgentId, availableAgents, activeSession, agents]);

  // A session outlives the permission that created it: the agent can be flipped
  // to personal, change owner, or drop this member from its allow-list, and the
  // server then refuses every send with `invocation_not_allowed` while still
  // serving the transcript (MUL-4525 — read uses the view gate, send re-runs the
  // invoke gate). `currentAgent` deliberately resolves an open session's agent
  // from the FULL list so the header stays honest, which means the picker filter
  // above cannot cover this case — judge the bound agent too (MUL-6380).
  const accessRevoked =
    currentAgent !== null &&
    !canAssignAgentToIssue(currentAgent, {
      userId: userId ?? null,
      role: memberRole,
    }).allowed;

  const availability = useWorkspaceAgentAvailability();
  const presenceDetail = useAgentPresence(wsId, currentAgent?.id);
  const presenceAvailability =
    presenceDetail === "loading" ? undefined : presenceDetail.availability;
  const isArchived = activeSession?.status === "archived";
  const runtimeBound =
    currentAgent !== null && isAgentRuntimeBound(currentAgent);
  const sending = !!pendingTask?.task_id;

  // ── Drafts ─────────────────────────────────────────────────────────────
  const draftKey = activeSessionId ?? DRAFT_NEW_SESSION;
  const draft = useChatDraftsStore((s) => s.drafts[draftKey] ?? "");
  const setDraft = useChatDraftsStore((s) => s.setDraft);
  const clearDraft = useChatDraftsStore((s) => s.clearDraft);
  const promoteNewDraft = useChatDraftsStore((s) => s.promoteNewDraft);

  // ── Realtime ───────────────────────────────────────────────────────────
  useChatSessionRealtime(activeSessionId, () => {
    setActiveSessionId(null);
  });

  // Exit text-selection mode whenever the chat tab loses focus. Expo
  // Router bottom tabs stay mounted across tab switches, so a plain
  // useEffect cleanup wouldn't fire — useFocusEffect is the navigation-
  // aware equivalent.
  useFocusEffect(
    useCallback(() => () => useChatSelectStore.getState().clear(), []),
  );

  // ── Auto markRead while viewing a session with unread state ──────────
  const isFocused = useIsFocused();
  const markRead = useMarkChatSessionRead();
  useEffect(() => {
    if (!isFocused) return;
    if (!activeSessionId) return;
    if (!activeSession?.has_unread) return;
    markRead.mutate(activeSessionId);
  }, [isFocused, activeSessionId, activeSession?.has_unread, markRead]);

  // ── Mutations ──────────────────────────────────────────────────────────
  const createSession = useCreateChatSession();
  const deleteSession = useDeleteChatSession();

  // ── Send burst ─────────────────────────────────────────────────────────
  const sessionPromiseRef = useRef<Promise<string | null> | null>(null);

  const ensureSession = useCallback(
    async (titleSeed: string): Promise<string | null> => {
      if (activeSessionId) return activeSessionId;
      if (!currentAgent) return null;
      if (sessionPromiseRef.current) return sessionPromiseRef.current;

      const promise = (async () => {
        try {
          const session = await createSession.mutateAsync({
            agent_id: currentAgent.id,
            title: titleSeed.slice(0, 50),
          });
          return session.id;
        } finally {
          sessionPromiseRef.current = null;
        }
      })();
      sessionPromiseRef.current = promise;
      return promise;
    },
    [activeSessionId, currentAgent, createSession],
  );

  const handleSend = useCallback(
    async (
      content: string,
      attachmentIds: string[] = [],
      options: { clearDraft?: boolean } = {},
    ) => {
      if (!currentAgent) return;
      // Invoke permission was revoked while this session was open — the server
      // would refuse before persisting anything. The composer is disabled in
      // this state; this is the belt-and-braces guard.
      if (accessRevoked) {
        Alert.alert(
          copy.permissionAlertTitle,
          copy.permissionAlertDescription,
        );
        return;
      }
      if (!runtimeBound) {
        Alert.alert(
          copy.runtimeRequiredTitle,
          copy.runtimeRequiredAlertDescription,
        );
        return;
      }

      const isNewSession = !activeSessionId;
      let sessionId: string | null;
      try {
        sessionId = await ensureSession(content);
      } catch (err) {
        // Session create runs the same invoke gate as a send, so a permission
        // change refuses here too — and this is the only layer that sees the
        // reason code (MUL-6380).
        Alert.alert(
          copy.messageNotSent,
          sendFailureMessage(err, copy.sendFailure),
        );
        throw err;
      }
      if (!sessionId) return;

      const sentAt = new Date().toISOString();
      const optimistic: ChatMessage = {
        id: `optimistic-${Date.now()}`,
        chat_session_id: sessionId,
        role: "user",
        content,
        task_id: null,
        created_at: sentAt,
      };
      const optimisticTaskId = `optimistic-${optimistic.id}`;
      qc.setQueryData<ChatMessage[]>(chatKeys.messages(sessionId), (old) =>
        old ? [...old, optimistic] : [optimistic],
      );
      qc.setQueryData<ChatPendingTask>(
        chatKeys.pendingTask(sessionId),
        (old) =>
          enqueuePendingChatTask(
            old,
            {
              task_id: optimisticTaskId,
              status: "queued",
              created_at: sentAt,
              message_id: optimistic.id,
              content,
            },
            Boolean(old?.task_id),
          ),
      );
      if (isNewSession) {
        promoteNewDraft(sessionId);
        setActiveSessionId(sessionId);
      }

      try {
        const result = await api.sendChatMessage(sessionId, content, {
          attachmentIds: attachmentIds.length > 0 ? attachmentIds : undefined,
        });
        // Replace the local bubble before reconciling pending state. When the
        // server says this is a follow-up, its real message id lets the shared
        // queue filter hide it immediately instead of waiting for the refetch.
        qc.setQueryData<ChatMessage[]>(chatKeys.messages(sessionId), (old) =>
          old?.map((message) =>
            message.id === optimistic.id
              ? {
                  ...message,
                  id: result.message_id,
                  task_id: result.task_id,
                  created_at: result.created_at,
                }
              : message,
          ),
        );
        seedAcceptedPendingTask(qc, {
          chat_session_id: sessionId,
          task_id: result.task_id,
          created_at: result.created_at,
          message_id: result.message_id,
          content,
          optimistic_task_id: optimisticTaskId,
          supports_queue: result.supports_queue,
          queued: result.queued,
        });
        qc.invalidateQueries({ queryKey: chatKeys.messages(sessionId) });
        if (options.clearDraft !== false) {
          clearDraft(sessionId);
        }
      } catch (err) {
        qc.setQueryData<ChatMessage[]>(chatKeys.messages(sessionId), (old) =>
          old ? old.filter((m) => m.id !== optimistic.id) : old,
        );
        qc.setQueryData<ChatPendingTask>(
          chatKeys.pendingTask(sessionId),
          (old) => removePendingChatTask(old, optimisticTaskId),
        );
        // The composer restores the draft on a thrown rejection but says nothing
        // about it, so a revoked-permission 403 used to read as a silent no-op
        // (MUL-6380). Name the cause here: only this layer sees the error body.
        Alert.alert(
          copy.messageNotSent,
          sendFailureMessage(err, copy.sendFailure),
        );
        throw err;
      }
    },
    [
      activeSessionId,
      currentAgent,
      accessRevoked,
      runtimeBound,
      ensureSession,
      qc,
      promoteNewDraft,
      clearDraft,
      copy,
    ],
  );

  // ── Cancel in-flight ───────────────────────────────────────────────────
  const handleStop = useCallback(() => {
    if (!pendingTask?.task_id || !activeSessionId) return;
    if (pendingTask.status === "queued") return;
    const taskId = pendingTask.task_id;
    const sessionId = activeSessionId;
    qc.setQueryData<ChatPendingTask>(chatKeys.pendingTask(sessionId), (old) =>
      removePendingChatTask(old, taskId),
    );
    void api.cancelTaskById(taskId)
      .catch(() => {
        // Silent — task may have already terminated server-side.
      })
      .finally(() => invalidatePendingTask(qc, sessionId));
  }, [pendingTask?.task_id, pendingTask?.status, activeSessionId, qc]);

  // ── Header / sheet actions ─────────────────────────────────────────────
  const handleNewChat = useCallback(() => {
    if (availableAgents.length > 1) {
      setAgentPickerOpen(true);
      return;
    }
    setSelectedAgentId(null);
    setActiveSessionId(null);
  }, [availableAgents.length]);

  const handlePickAgent = useCallback((agent: Agent) => {
    setSelectedAgentId(agent.id);
    setActiveSessionId(null);
  }, []);

  // Apply the user's pick from the chat-sessions route (or "no session"
  // when they delete the active one in the sheet).
  useEffect(() => {
    if (!selectRequest) return;
    setSelectedAgentId(null);
    setActiveSessionId(selectRequest.id);
    consumeSelect();
  }, [selectRequest, consumeSelect]);

  const handleDeleteActive = useCallback(() => {
    if (!activeSession) return;
    Alert.alert(
      copy.deleteChatTitle,
      copy.deleteChatDescription(
        chatSessionDisplayTitle(activeSession.title, copy.newChat),
      ),
      [
        { text: copy.cancel, style: "cancel" },
        {
          text: copy.delete,
          style: "destructive",
          onPress: () => {
            const id = activeSession.id;
            setActiveSessionId(null);
            deleteSession.mutate(id);
          },
        },
      ],
      { cancelable: true },
    );
  }, [activeSession, copy, deleteSession]);

  // ── Composer disabled-state ────────────────────────────────────────────
  const disabled =
    !currentAgent ||
    accessRevoked ||
    availability === "none" ||
    isArchived === true ||
    !runtimeBound;
  const disabledReason = !currentAgent
    ? copy.noAgentSelected
    : accessRevoked
      ? copy.accessRevoked
      : availability === "none"
        ? copy.noAgentsWorkspace
        : isArchived
          ? copy.archivedChat
          : !runtimeBound
            ? copy.agentNeedsRuntime
          : undefined;

  return (
    <View className="flex-1 bg-background">
      <Header
        center={
          <ChatTitleButton
            currentSession={activeSession}
            currentAgent={currentAgent}
            onPress={() => {
              if (!wsSlug) return;
              router.push({
                pathname: "/[workspace]/chat-sessions",
                params: { workspace: wsSlug },
              });
            }}
          />
        }
        right={
          <ChatSessionActions
            showMore={!!activeSession}
            onMorePress={handleDeleteActive}
            onNewPress={handleNewChat}
          />
        }
      />
      {availability === "none" ? <NoAgentBanner /> : null}
      <KeyboardAvoidingView
        behavior={Platform.OS === "ios" ? "padding" : undefined}
        className="flex-1"
      >
        <ChatMessageList
          messages={visibleMessages}
          loading={messagesLoading}
          hasSessions={sessions.length > 0}
          agent={currentAgent}
          onPickPrompt={(text) => setDraft(draftKey, text)}
          onQuickAction={(action) =>
            handleSend(action.prompt, [], { clearDraft: false })
          }
          quickActionsDisabled={sending || disabled}
          pendingTask={pendingTask}
          liveTaskMessages={liveTaskMessages}
          availability={presenceAvailability}
        />
        {runtimeBound ? (
          <OfflineBanner
            agentName={currentAgent?.name}
            availability={presenceAvailability}
          />
        ) : currentAgent ? (
          <RuntimeRequiredBanner agentName={currentAgent.name} />
        ) : null}
        <ChatComposer
          value={draft}
          onChangeText={(next) => setDraft(draftKey, next)}
          onSend={handleSend}
          onStop={handleStop}
          sending={sending}
          allowStop={pendingTask?.status !== "queued"}
          disabled={disabled}
          disabledReason={disabledReason}
        />
      </KeyboardAvoidingView>

      <AgentPickerSheet
        visible={agentPickerOpen}
        agents={availableAgents}
        currentAgentId={currentAgent?.id ?? null}
        onPick={handlePickAgent}
        onClose={() => setAgentPickerOpen(false)}
      />
    </View>
  );
}
