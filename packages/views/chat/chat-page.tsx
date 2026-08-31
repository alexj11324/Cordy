"use client";

import { useEffect, useRef, useState } from "react";
import { ArrowLeft, MoreHorizontal } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@patchbay/ui/components/ui/button";
import { useIsCompact } from "@patchbay/ui/hooks/use-mobile";
import { useWorkspacePaths } from "@patchbay/core/paths";
import { useChatStore } from "@patchbay/core/chat";
import { chatQuickActionsPendingOptions } from "@patchbay/core/chat/queries";
import { useRegenerateChatQuickActions } from "@patchbay/core/chat/mutations";
import { useQuickActionsPendingTimeout } from "@patchbay/core/chat/use-quick-actions-pending-timeout";
import { useQuickActionsFailureToast } from "./components/use-quick-actions-failure-toast";
import { useQuery } from "@tanstack/react-query";
import type { Agent, ChatSession } from "@patchbay/core/types";
import { PageHeader } from "../layout/page-header";
import { useNavigation } from "../navigation";
import { useT } from "../i18n";
import { ActorAvatar } from "../common/actor-avatar";
import { ChatMessageList, ChatMessageSkeleton } from "./components/chat-message-list";
import { ChatInput } from "./components/chat-input";
import { ChatQueue } from "./components/chat-queue";
import { ChatThreadList } from "./components/chat-thread-list";
import { ChatSessionHeader } from "./components/chat-session-header";
import { EmptyState } from "./components/chat-empty-state";
import { NewChatButton } from "./components/new-chat-button";
import { useChatController } from "./components/use-chat-controller";
import { OfflineBanner } from "./components/offline-banner";
import { NoAgentBanner } from "./components/no-agent-banner";
import { ArchivedAgentBanner } from "./components/archived-agent-banner";
import { AgentAccessRevokedBanner } from "./components/agent-access-revoked-banner";
import { RuntimeRequiredBanner } from "./components/runtime-required-banner";

/**
 * Chat tab — the first-class Agent conversation workspace. Desktop keeps the
 * topic history in the global Agent sidebar while compact layouts retain the
 * list/conversation toggle. All conversation logic is shared with the
 * floating FAB via `useChatController`.
 *
 * Selection is URL-addressable via `?session=<id>` so a thread can be
 * deep-linked, opened from a notification, and survive refresh. The chat
 * store's `activeSessionId` stays the source of truth (both surfaces read
 * it); the URL is kept in sync in both directions. `?agent=<id>` is the
 * complementary one-shot deep link for a NEW chat: it starts a fresh compose
 * bound to that agent and is then stripped from the URL.
 *
 * Starting a chat is where the agent is chosen: the header ⊕ opens an agent
 * picker (see NewChatButton), so the compose box no longer needs its own
 * agent selector. Unlike the FAB, this page passes no `contextItems` to
 * `ChatInput`, so its `@` mentions fall back to manual search (issue-comment
 * style).
 */
export function ChatPage() {
  const { t } = useT("chat");
  const { searchParams, replace } = useNavigation();
  const wsPaths = useWorkspacePaths();
  const isCompact = useIsCompact();

  const c = useChatController({ isActive: true });
  const { data: quickActionsPending = null } = useQuery(
    chatQuickActionsPendingOptions(c.activeSessionId ?? ""),
  );
  // Drop a stuck pending marker (dead daemon / failed supplement) so the pill
  // spinner stops and a later refresh starts clean (PB-5149).
  useQuickActionsPendingTimeout(c.activeSessionId ?? null, quickActionsPending);
  // Toast when an accepted refresh later fails in the daemon (async half).
  useQuickActionsFailureToast(c.activeSessionId ?? null);
  const regenerateQuickActions = useRegenerateChatQuickActions();
  const urlSession = searchParams.get("session") || null;
  const urlAgent = searchParams.get("agent") || null;

  // "Composing a brand-new chat" — the user hit ⊕ but hasn't sent yet, so no
  // session exists. At compact widths this decides list-vs-conversation; on desktop the
  // conversation pane is always mounted so it only needs to reset itself once a
  // real session takes over.
  const [composingNew, setComposingNew] = useState(false);
  useEffect(() => {
    // Read the LIVE store value for the same reason as the session sync
    // effects below: under StrictMode's double-invoke this effect replays
    // with the render-captured snapshot, and a stale non-null session (a
    // persisted chat the URL→store effect already cleared) would revert the
    // composingNew=true that the later `?agent=` intent effect just set.
    if (useChatStore.getState().activeSessionId) setComposingNew(false);
  }, [c.activeSessionId]);

  // Two-way sync between the URL (`?session=`) and the chat store's
  // activeSessionId. Both effects read the LIVE store value via
  // `useChatStore.getState()` rather than the render-captured `c.activeSessionId`.
  // That is what keeps them from fighting on mount: a naive mirror effect fires
  // with the stale (null) snapshot and "corrects" the URL by stripping the
  // session before the URL→store effect has applied — breaking deep links and
  // making selection / new-chat feel unresponsive. Reading getState() sees the
  // value the sibling effect just wrote, so the reconciliation converges in one
  // pass and is idempotent under StrictMode's double-invoke.

  // URL → store: deep link, refresh, notification click, back/forward.
  useEffect(() => {
    if (urlSession !== useChatStore.getState().activeSessionId) {
      c.setActiveSession(urlSession);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps -- react to URL only
  }, [urlSession]);

  // store → URL: thread selection, "new chat", and sessions created by sending.
  useEffect(() => {
    const live = useChatStore.getState().activeSessionId;
    const current = searchParams.get("session") || null;
    if (live !== current) {
      const base = wsPaths.chat();
      replace(live ? `${base}?session=${live}` : base);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps -- react to store only
  }, [c.activeSessionId]);

  // `?agent=` intent bookkeeping. The ref holds the param value already
  // consumed (or superseded) so the effect below fires at most once per deep
  // link — it also bridges the async window between replace() and the
  // searchParams actually dropping the param. Any explicit user action must
  // supersede a still-pending intent: agent/member queries can resolve late,
  // and a deferred intent firing after the user picked a thread (or started
  // another chat) would clobber that choice.
  const consumedAgentIntent = useRef<string | null>(null);
  const supersedeAgentIntent = () => {
    if (urlAgent) consumedAgentIntent.current = urlAgent;
  };

  const handleSelect = (session: ChatSession) => {
    supersedeAgentIntent();
    c.handleSelectSession(session);
    setComposingNew(false);
  };

  // Single archive path for both entry points (thread-list row + conversation
  // header). When the archived chat is the one in view, move the pane off it:
  // on desktop advance to the next chat (Inbox-style); when compact drop back to
  // the list, which reads more naturally than being thrown into an unrelated
  // conversation full-screen. Archiving any other chat leaves the view put.
  const handleArchive = (session: ChatSession) => {
    supersedeAgentIntent();
    if (session.id === c.activeSessionId) {
      if (isCompact) {
        c.setActiveSession(null);
        setComposingNew(false);
      } else {
        c.advanceSelectionAfterArchive(session);
      }
    }
    c.archiveSession(session.id);
  };

  const startNewChat = (agent: Agent | null) => {
    // A manual ⊕ pick outranks a pending deep link; when called FROM the
    // intent effect the ref is already set to this param, so this is a no-op.
    supersedeAgentIntent();
    if (agent) c.handleStartNewChat(agent);
    else c.handleNewChat();
    setComposingNew(true);
  };

  const changeProjectContext = (projectId: string | null) => {
    if (projectId === c.activeProjectId) return;
    c.handleProjectChange(projectId);
    // Removing a project stays in the current conversation. Choosing a
    // project for an existing conversation starts a clean session, and a
    // compact layout must stay in the compose pane after activeSessionId is
    // cleared.
    if (!c.currentSession || projectId !== null) setComposingNew(true);
  };

  // URL → new chat: `?agent=<id>` is the deep link used by "DM" entry points
  // (e.g. the agent detail page) to land on a fresh compose bound to that
  // agent. The permission-filtered agent list loads async, so the intent is
  // consumed on the render where the agent resolves, then the param is
  // stripped so refresh / the session sync above don't replay it. The ref
  // resets once the param is gone so a later identical deep link fires again.
  // A settled miss (access revoked, agent archived, bad id) is a denial: it
  // explains itself with a toast and consumes the intent so a later refetch
  // that surfaces the agent cannot start a chat without a fresh click. While
  // the queries are still loading the intent simply stays pending.
  useEffect(() => {
    if (!urlAgent) {
      consumedAgentIntent.current = null;
      return;
    }
    if (consumedAgentIntent.current === urlAgent) return;
    const agent = c.availableAgents.find((a) => a.id === urlAgent);
    if (agent) {
      consumedAgentIntent.current = urlAgent;
      startNewChat(agent);
      replace(wsPaths.chat());
      return;
    }
    if (c.agentsSettled) {
      consumedAgentIntent.current = urlAgent;
      toast.error(t(($) => $.page.agent_link_no_access));
      replace(wsPaths.chat());
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps -- consume when the URL param or the resolving agent list changes
  }, [urlAgent, c.availableAgents, c.agentsSettled]);

  const newChatButton = (
    <NewChatButton
      agents={c.availableAgents}
      userId={c.user?.id}
      onStart={startNewChat}
      side="bottom"
    />
  );

  const listHeader = (
    <PageHeader>
      <h1 className="flex-1 text-body font-semibold">{t(($) => $.page.title)}</h1>
      {newChatButton}
    </PageHeader>
  );

  const listBody = (
    <div className="px-2 py-1">
      <ChatThreadList
        sessions={c.sessions}
        agents={c.agents}
        activeSessionId={c.activeSessionId}
        onSelectSession={handleSelect}
        onArchive={handleArchive}
      />
    </div>
  );

  // The conversation pane: message list / skeleton / empty above a persistent
  // banner + input. Identical composition to the floating window's body, so a
  // brand-new chat (no active session) shows the agent-aware empty state + input.
  // The composer shows the bound agent's avatar; switching agents still starts
  // from ⊕. `@container`: the conversation column's gutter (CHAT_GUTTER) widens
  // with THIS pane, which the user resizes independently of the browser window.
  const queuedTasks = c.pendingTask?.queued_tasks ?? [];
  const conversation = (
    <div className="flex flex-1 flex-col min-h-0 @container">
      {c.currentSession ? (
        <ChatSessionHeader
          session={c.currentSession}
          agent={c.activeAgent}
          onArchive={handleArchive}
        />
      ) : (
        <div className="flex h-12 shrink-0 items-center gap-1 border-b border-border/70 px-6">
          <h1 className="text-body font-semibold">{t(($) => $.navigation.new_topic)}</h1>
          <button
            type="button"
            aria-label={t(($) => $.navigation.topic_actions)}
            className="inline-flex size-7 items-center justify-center rounded-md text-muted-foreground outline-none transition-colors hover:bg-accent hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring"
          >
            <MoreHorizontal className="size-4" />
          </button>
        </div>
      )}
      {c.showSkeleton ? (
        <ChatMessageSkeleton />
      ) : c.hasMessages ? (
        <ChatMessageList
          key={c.activeSessionId}
          messages={c.messages}
          agentId={c.activeAgent?.id}
          agentName={c.activeAgent?.name}
          userId={c.user?.id}
          userName={c.user?.name}
          pendingTask={c.pendingTask}
          availability={c.availability}
          firstItemIndex={c.firstItemIndex}
          hasOlderMessages={c.hasOlderMessages}
          isFetchingOlderMessages={c.isFetchingOlderMessages}
          onLoadOlderMessages={() => void c.fetchOlderMessages()}
          onQuickAction={(action) => c.handleSend(action.prompt)}
          quickActionsDisabled={
            !!c.pendingTaskId ||
            c.isSessionArchived ||
            c.isAgentArchived ||
            c.isAgentAccessRevoked ||
            !c.isAgentRuntimeBound ||
            c.noAgent
          }
          onRegenerateQuickActions={(message) =>
            c.activeSessionId
              ? regenerateQuickActions.mutateAsync({
                  sessionId: c.activeSessionId,
                  messageId: message.id,
                })
              : undefined
          }
          quickActionsPendingMessageId={quickActionsPending?.message_id ?? null}
        />
      ) : (
        <EmptyState agent={c.activeAgent} />
      )}

      {c.isAgentAccessRevoked ? (
        <AgentAccessRevokedBanner agentName={c.activeAgent?.name} />
      ) : c.noAgent ? (
        <NoAgentBanner />
      ) : c.isAgentArchived ? (
        <ArchivedAgentBanner agentName={c.activeAgent?.name} />
      ) : !c.isAgentRuntimeBound && c.activeAgent ? (
        <RuntimeRequiredBanner
          agentId={c.activeAgent.id}
          agentName={c.activeAgent.name}
        />
      ) : (
        <OfflineBanner agentName={c.activeAgent?.name} availability={c.availability} />
      )}

      <ChatInput
        onSend={c.handleSend}
        restoreDraftRequest={c.restoreDraftRequest}
        onRestoreDraftApplied={c.handleRestoreDraftApplied}
        uploadEnabled={c.uploadEnabled && !c.isAgentAccessRevoked}
        onStop={c.handleStop}
        isRunning={!!c.pendingTaskId}
        allowSubmitWhileRunning={c.pendingTask?.supports_queue === true}
        queueSlot={
          <ChatQueue
            tasks={queuedTasks}
            headStatus={c.pendingTask?.status}
            onSendNow={c.handleSendQueuedTaskNow}
            sendNowDisabled={c.isAgentAccessRevoked}
            onEdit={c.handleEditQueuedTask}
            onRemove={c.handleRemoveQueuedTask}
            onClear={c.handleClearQueuedTasks}
          />
        }
        disabled={
          c.isSessionArchived ||
          c.isAgentArchived ||
          c.isAgentAccessRevoked ||
          !c.isAgentRuntimeBound
        }
        noAgent={c.noAgent}
        agentArchived={c.isAgentArchived}
        agentAccessRevoked={c.isAgentAccessRevoked}
        agentRuntimeRequired={!c.isAgentRuntimeBound}
        agentName={c.activeAgent?.name}
        leftAdornment={
          c.activeAgent ? (
            <ActorAvatar
              actorType="agent"
              actorId={c.activeAgent.id}
              size="lg"
              profileLink={false}
              showStatusDot
            />
          ) : null
        }
        projects={c.projects}
        projectId={c.activeProjectId}
        projectContextUnsupported={c.projectContextUnsupported}
        onProjectChange={changeProjectContext}
        isProjectUpdating={c.isProjectUpdating}
        focusRequest={c.focusInputRequest}
      />
    </div>
  );

  // -- Compact: list / conversation toggle -----------------------------------
  if (isCompact) {
    if (c.activeSessionId || composingNew) {
      return (
        <div className="flex flex-1 flex-col min-h-0">
          <div className="flex h-12 shrink-0 items-center border-b px-2">
            <Button
              variant="ghost"
              size="sm"
              onClick={() => {
                c.setActiveSession(null);
                setComposingNew(false);
              }}
              className="gap-1.5 text-muted-foreground"
            >
              <ArrowLeft className="h-4 w-4" />
              {t(($) => $.page.title)}
            </Button>
          </div>
          {conversation}
        </div>
      );
    }
    return (
      <div className="flex flex-1 flex-col min-h-0">
        {listHeader}
        <div className="flex-1 min-h-0 overflow-y-auto">{listBody}</div>
      </div>
    );
  }

  // -- Desktop: the global sidebar owns agent selection and topic history, so
  // the conversation is a single LobeHub-style workspace canvas here. This is
  // what lets the empty state remain useful on first load instead of showing
  // a blank detail pane waiting for a second click.
  return <div className="flex flex-1 min-h-0">{conversation}</div>;
}
