"use client";

import {
  memo,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { toast } from "sonner";
import { useQuery } from "@tanstack/react-query";
import { Virtuoso, type Components } from "react-virtuoso";
import { cn } from "@patchbay/ui/lib/utils";
import { Skeleton } from "@patchbay/ui/components/ui/skeleton";
import { Button } from "@patchbay/ui/components/ui/button";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@patchbay/ui/components/ui/collapsible";
import {
  Tooltip,
  TooltipTrigger,
  TooltipContent,
} from "@patchbay/ui/components/ui/tooltip";
import {
  ChevronRight,
  ChevronDown,
  AlertCircle,
  AlertTriangle,
  ArrowUpRight,
  Copy,
  RotateCw,
} from "lucide-react";
import { useScrollFade } from "@patchbay/ui/hooks/use-scroll-fade";
import { isTaskMessageTaskId, taskMessagesOptions } from "@patchbay/core/chat/queries";
import { RichContent } from "../../rich-content";
import { RichContentScrollRootProvider } from "../../rich-content/scroll-root";
import { copyText } from "@patchbay/ui/lib/clipboard";
import { AttachmentList } from "../../issues/components/comment-card";
import { ImageSequenceProvider } from "../../editor";
import { collectImageSequence } from "@patchbay/core/attachments/image-sequence";
import type { AgentAvailability } from "@patchbay/core/agents";
import { resolveFailureReasonKey } from "@patchbay/core/agents";
import type {
  ChatMessage,
  ChatPendingTask,
  ChatQuickAction,
  TaskMessagePayload,
} from "@patchbay/core/types";
import type { ChatTimelineItem } from "@patchbay/core/chat";
import { buildTimeline } from "../../common/agent-thread-events";
import { redactSecrets } from "../../common/agent-thread-events/redact";
import { traceEventSummary } from "../../common/agent-thread-events/trace-event-presenter";
import { OnboardingStarterCards } from "./onboarding-starter-cards";
import { TaskStatusPill } from "./task-status-pill";
import { CHAT_COLUMN, CHAT_GUTTER } from "./chat-column";
import { formatElapsedMs } from "../lib/format";
import { extractCopyText } from "../lib/copy-text";
import { stripChatQuickActionsProtocol } from "../lib/quick-actions";
import { ActorAvatar } from "../../common/actor-avatar";
import { useLocale, useT } from "../../i18n";

// ─── Public component ────────────────────────────────────────────────────

interface ChatMessageListProps {
  messages: ChatMessage[];
  /** Identity shown in the LobeHub-style assistant message header. */
  agentId?: string | null;
  agentName?: string | null;
  /** Identity shown in the right-aligned member message header. */
  userId?: string | null;
  userName?: string | null;
  /**
   * Optional per-message identity overrides for embedded conversations where
   * more than one person (or another agent) can author the user-side turns.
   * Direct Chat leaves this unset and keeps its single-member identity.
   */
  messageActors?: Readonly<
    Record<
      string,
      {
        actorType: "member" | "agent";
        actorId?: string | null;
        actorName?: string | null;
      }
    >
  >;
  /**
   * Server-authoritative pending-task snapshot. `null` / undefined means
   * no in-flight task — list renders without StatusPill.
   */
  pendingTask: ChatPendingTask | null | undefined;
  /** Resolved presence; pass `undefined` while loading to keep the pill copy neutral. */
  availability: AgentAvailability | undefined;
  firstItemIndex?: number;
  hasOlderMessages?: boolean;
  isFetchingOlderMessages?: boolean;
  onLoadOlderMessages?: () => void;
  /** Transform assistant task text for embedded chat protocols before render/copy. */
  transformContent?: (content: string) => string;
  /** Send the full hidden prompt behind an assistant follow-up chip. */
  onQuickAction?: (action: ChatQuickAction) => void | Promise<unknown>;
  quickActionsDisabled?: boolean;
  /**
   * Regenerate the follow-up suggestions for the session's latest assistant
   * turn (the "refresh" affordance, PB-5149). Only offered on that turn —
   * regeneration resumes the newest provider state, so an older turn's pills
   * can't be refreshed in place.
   */
  onRegenerateQuickActions?: (message: ChatMessage) => void | Promise<unknown>;
  /**
   * Message currently awaiting its quick-actions supplement (client-only
   * marker raised by chat:done or a refresh) — renders pill skeletons under
   * that reply until chat:quick_actions resolves it.
   */
  quickActionsPendingMessageId?: string | null;
}

// ─── Virtuoso chrome ─────────────────────────────────────────────────────
//
// Header/Footer MUST be stable component references (module scope), never
// inline arrows in the `components` prop: an inline `components={{ Footer:
// () => … }}` creates a new component *type* every render, so React unmounts
// and remounts the whole Header/Footer subtree each time. During task
// streaming that tore down and rebuilt the entire live timeline — every row
// and every Markdown parse — on every `task:message` event, freezing the
// renderer for seconds at a time (PB-3960). Per-render data flows through
// Virtuoso's `context` prop instead, which reaches these components as an
// ordinary prop (re-render, not remount).

interface ChatListContext {
  isFetchingOlderMessages: boolean;
  showStatusPill: boolean;
  pendingTask: ChatPendingTask | null | undefined;
  liveTaskMessages: readonly TaskMessagePayload[] | undefined;
  availability: AgentAvailability | undefined;
}

/**
 * One Virtuoso row. A live (still-streaming) task and the persisted assistant
 * message it becomes share ONE key — `task:<taskId>` — so the handoff replaces
 * this item's data in place instead of unmounting a Footer subtree and mounting
 * a different row (PB-4922). That identity is what keeps an already-rendered
 * Mermaid diagram or HTML iframe mounted across task completion.
 */
type ChatRenderItem =
  | { key: string; kind: "message"; message: ChatMessage; taskId: string | null }
  | { key: string; kind: "live"; taskId: string; createdAt?: string };

const messageTimeFormatters = new Map<string, Intl.DateTimeFormat>();

function formatMessageTime(
  createdAt: string | undefined,
  locale: string,
): string | null {
  if (!createdAt) return null;
  const date = new Date(createdAt);
  if (!Number.isFinite(date.getTime())) return null;

  let formatter = messageTimeFormatters.get(locale);
  if (!formatter) {
    formatter = new Intl.DateTimeFormat(locale, {
      hour: "2-digit",
      minute: "2-digit",
    });
    messageTimeFormatters.set(locale, formatter);
  }
  return formatter.format(date);
}

/**
 * Row key for a persisted message. Assistant turns carrying a task_id key on
 * the task so they can inherit the live row; everything else keys on its own
 * id.
 */
function messageRowKey(message: ChatMessage): string {
  return message.role === "assistant" && message.task_id
    ? `task:${message.task_id}`
    : message.id;
}

function ChatListHeader({ context }: { context?: ChatListContext }) {
  const { t } = useT("chat");
  return (
    <div className={cn(CHAT_COLUMN, "pt-4")}>
      {context?.isFetchingOlderMessages && (
        <div className="text-center text-caption text-muted-foreground">
          {t(($) => $.message_list.loading_older)}
        </div>
      )}
    </div>
  );
}

// The Footer now carries only the status pill — task chrome, not content. The
// live timeline moved into a real row so it can keep its identity when the
// task completes (see ChatRenderItem).
//
// The container always renders (even with no pill) so the list keeps a
// constant bottom inset: without it the last row's own py-2 was the only gap
// between the final reply (and its follow-up pills) and the composer.
function ChatListFooter({ context }: { context?: ChatListContext }) {
  return (
    <div className={cn(CHAT_COLUMN, "pb-4 space-y-4")}>
      {context?.showStatusPill && context.pendingTask ? (
        <TaskStatusPill
          pendingTask={context.pendingTask}
          taskMessages={context.liveTaskMessages ?? []}
          availability={context.availability}
        />
      ) : null}
    </div>
  );
}

const LIST_COMPONENTS: Components<ChatRenderItem, ChatListContext> = {
  Header: ChatListHeader,
  Footer: ChatListFooter,
};

export function ChatMessageList({
  messages,
  agentId,
  agentName,
  userId,
  userName,
  messageActors,
  pendingTask,
  availability,
  firstItemIndex = 0,
  hasOlderMessages = false,
  isFetchingOlderMessages = false,
  onLoadOlderMessages,
  transformContent,
  onQuickAction,
  quickActionsDisabled = false,
  onRegenerateQuickActions,
  quickActionsPendingMessageId = null,
}: ChatMessageListProps) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const [scrollContainerEl, setScrollContainerEl] = useState<HTMLDivElement | null>(null);
  const [isNearBottom, setIsNearBottom] = useState(true);
  const setScrollContainerRef = useCallback((node: HTMLDivElement | null) => {
    scrollRef.current = node;
    setScrollContainerEl(node);
  }, []);
  // Soft edge fade hinting more content above/below. Kept small so it barely
  // grazes full-bleed previews (image / HTML) at the edges.
  const fadeStyle = useScrollFade(scrollRef, 16);

  const pendingTaskId = pendingTask?.task_id ?? null;

  // The session's newest assistant turn — the only one whose quick actions can
  // be refreshed (regeneration resumes the newest provider state). Computed off
  // the persisted list so the affordance tracks the real tail, not a live row.
  const latestAssistantMessageId = useMemo(() => {
    for (let i = messages.length - 1; i >= 0; i--) {
      const m = messages[i];
      if (m && m.role === "assistant" && m.task_id) return m.id;
    }
    return null;
  }, [messages]);

  // Mika's onboarding opening self-describes (message_kind stamped by the
  // completion path — the hidden kickoff row never reaches clients) and
  // carries the product's starter cards instead of that turn's quick-action
  // chips (PB-5765).
  const starterCardsMessageId = useMemo(
    () =>
      messages.find(
        (m) => m.role === "assistant" && m.message_kind === "onboarding_opening",
      )?.id ?? null,
    [messages],
  );

  // Once the assistant message for this pending task has landed in the
  // messages list, AssistantMessage owns its rendering — suppress the live
  // timeline (and pill) to avoid rendering the same content in two places
  // during the invalidate → refetch window.
  const pendingAlreadyPersisted = !!pendingTaskId && messages.some(
    (m) => m.role === "assistant" && m.task_id === pendingTaskId,
  );

  // Live timeline for the in-flight task. useRealtimeSync keeps this cache
  // current via setQueryData on task:message events. Only used here to decide
  // whether the live row exists and to feed the status pill — the row itself
  // reads the same cache entry through AssistantMessage.
  const showLiveTimeline = !!pendingTaskId && !pendingAlreadyPersisted;
  const canFetchLiveTimeline = isTaskMessageTaskId(pendingTaskId) && !pendingAlreadyPersisted;
  const { data: liveTaskMessages } = useQuery({
    ...taskMessagesOptions(pendingTaskId ?? ""),
    enabled: canFetchLiveTimeline,
  });
  const hasLive = showLiveTimeline && (liveTaskMessages?.length ?? 0) > 0;
  const showStatusPill = !!pendingTaskId && !pendingAlreadyPersisted && !!pendingTask;

  // Persisted messages plus, while a task is in flight, one synthetic trailing
  // row for it. When the assistant message persists, `hasLive` goes false and
  // the message takes the SAME key at the SAME position — an in-place data
  // swap, not a remount. The onboarding kickoff is a server-authored carrier
  // for Mika's first task, not something the member typed, so it never becomes
  // a visible bubble.
  const renderItems: ChatRenderItem[] = useMemo(() => {
    const items: ChatRenderItem[] = messages
      .filter((message) => message.message_kind !== "onboarding_kickoff")
      .map((message) => ({
        key: messageRowKey(message),
        kind: "message" as const,
        message,
        taskId: message.task_id ?? null,
    }));
    if (hasLive && pendingTaskId) {
      items.push({
        key: `task:${pendingTaskId}`,
        kind: "live",
        taskId: pendingTaskId,
        createdAt: pendingTask?.created_at,
      });
    }
    return items;
  }, [messages, hasLive, pendingTask?.created_at, pendingTaskId]);

  const firstIndex = renderItems.length > 0 ? firstItemIndex : 0;

  const listContext: ChatListContext = {
    isFetchingOlderMessages,
    showStatusPill,
    pendingTask,
    liveTaskMessages,
    availability,
  };

  // Every image in this session, in message order, so opening one lets the
  // reader page through the rest (PB-5752). Built from the message data, not
  // from what Virtuoso currently has mounted.
  //
  // Persisted messages only: a task Agent event history's own attachments live behind a
  // separate query and its blocks are collapsed by default, so an image in
  // there keeps its standalone preview instead of entering a sequence the
  // reader can't see the rest of.
  const imageSequence = useMemo(
    () =>
      collectImageSequence(
        messages.map((message) => ({
          content: message.content,
          attachments: message.attachments,
        })),
      ),
    [messages],
  );

  return (
    <ImageSequenceProvider items={imageSequence}>
    <div
      ref={setScrollContainerRef}
      data-tab-scroll-root
      style={fadeStyle}
      // The gutter lives on the scroll container, so it applies once to the
      // whole list — rows, header, footer — and the scrollbar still rides the
      // surface edge rather than being inset with the text.
      className={cn("flex-1 overflow-y-auto", CHAT_GUTTER)}
    >
      {/* Already inside the gutter + column, so this pre-mount frame renders the
       *  skeleton BODY rather than <ChatMessageSkeleton>, which brings its own
       *  wrapper for use as a standalone sibling of the list. */}
      {!scrollContainerEl ? (
        <div className={cn(CHAT_COLUMN, "pt-4")}>
          <ChatSkeletonBody />
        </div>
      ) : (
      // Chat scrolls inside its own element, so rich blocks must measure
      // "near-viewport" against that element rather than the browser viewport —
      // otherwise a diagram only starts loading once it is already on screen.
      <RichContentScrollRootProvider scrollRoot={scrollContainerEl}>
      <Virtuoso
        customScrollParent={scrollContainerEl}
        data={renderItems}
        firstItemIndex={firstIndex}
        // Open pinned to the newest message. The list is remounted per session
        // (`key={activeSessionId}` upstream), so this initial position is
        // re-applied on every session switch. Without it a fresh Virtuoso
        // renders from the top and the only thing that can scroll it down is
        // `followOutput`, which reacts to post-mount data growth — leaving the
        // landing spot racy: cached sessions resolve synchronously and stick at
        // the top, while fetched ones sometimes catch a growth tick and land at
        // the bottom. `align: "end"` bottom-aligns even a last message taller
        // than the viewport, so switching sessions always shows the latest reply.
        initialTopMostItemIndex={{ index: "LAST", align: "end" }}
        increaseViewportBy={{ top: 400, bottom: 600 }}
        atBottomThreshold={120}
        atBottomStateChange={setIsNearBottom}
        followOutput={() => (!isFetchingOlderMessages && isNearBottom ? "smooth" : false)}
        startReached={() => {
          if (hasOlderMessages && !isFetchingOlderMessages) {
            onLoadOlderMessages?.();
          }
        }}
        computeItemKey={(_, item) => item.key}
        context={listContext}
        components={LIST_COMPONENTS}
        itemContent={(_, item) => (
          <div className={cn(CHAT_COLUMN, "py-2")}>
            <MessageBubble
              item={item}
              agentId={agentId}
              agentName={agentName}
              userId={userId}
              userName={userName}
              messageActor={
                item.kind === "message" ? messageActors?.[item.message.id] : undefined
              }
              isPending={!!pendingTaskId && item.taskId === pendingTaskId}
              transformContent={transformContent}
              onQuickAction={onQuickAction}
              quickActionsDisabled={quickActionsDisabled}
              onRegenerateQuickActions={onRegenerateQuickActions}
              latestAssistantMessageId={latestAssistantMessageId}
              quickActionsPendingMessageId={quickActionsPendingMessageId}
              starterCardsMessageId={starterCardsMessageId}
            />
          </div>
        )}
      />
      </RichContentScrollRootProvider>
      )}
    </div>
    </ImageSequenceProvider>
  );
}

/**
 * Placeholder shown while `chat_message` for a session is being fetched
 * (initial refresh, or switching to an un-cached session). Shape roughly
 * mirrors an assistant → user → assistant exchange so the window doesn't
 * shift under the user when real messages arrive.
 */
export function ChatMessageSkeleton() {
  return (
    <div className={cn("flex-1 overflow-hidden", CHAT_GUTTER)}>
      <div className={cn(CHAT_COLUMN, "py-4")}>
        <ChatSkeletonBody />
      </div>
    </div>
  );
}

// The rows themselves, so the list's pre-mount frame can drop them straight
// into the gutter + column it already established.
function ChatSkeletonBody() {
  return (
    <div className="space-y-5">
      <div className="space-y-2">
        <Skeleton className="h-3.5 w-3/4" />
        <Skeleton className="h-3.5 w-1/2" />
      </div>
      <div className="flex justify-end">
        <Skeleton className="h-8 w-48 rounded-2xl" />
      </div>
      <div className="space-y-2">
        <Skeleton className="h-3.5 w-2/3" />
        <Skeleton className="h-3.5 w-5/6" />
        <Skeleton className="h-3.5 w-1/3" />
      </div>
    </div>
  );
}

// ─── Message bubbles ─────────────────────────────────────────────────────

// memo: every streamed task:message re-renders ChatMessageList, and with it
// every VISIBLE row via itemContent. Message objects are referentially
// stable for unchanged messages and isPending is a boolean, so a shallow
// memo skips reconciling rows the stream didn't touch — the persisted
// history stays inert while only the live row updates.
const MessageBubble = memo(function MessageBubble({
  item,
  agentId,
  agentName,
  userId,
  userName,
  messageActor,
  isPending,
  transformContent,
  onQuickAction,
  quickActionsDisabled,
  onRegenerateQuickActions,
  latestAssistantMessageId,
  quickActionsPendingMessageId,
  starterCardsMessageId,
}: {
  item: ChatRenderItem;
  agentId?: string | null;
  agentName?: string | null;
  userId?: string | null;
  userName?: string | null;
  messageActor?: {
    actorType: "member" | "agent";
    actorId?: string | null;
    actorName?: string | null;
  };
  isPending: boolean;
  transformContent?: (content: string) => string;
  onQuickAction?: (action: ChatQuickAction) => void | Promise<unknown>;
  quickActionsDisabled: boolean;
  onRegenerateQuickActions?: (message: ChatMessage) => void | Promise<unknown>;
  latestAssistantMessageId: string | null;
  quickActionsPendingMessageId: string | null;
  starterCardsMessageId: string | null;
}) {
  // The live row and the persisted assistant row both land here under one key,
  // and both render <AssistantMessage> — same component type, same position —
  // so React reconciles rather than remounts at task completion.
  if (item.kind === "live") {
    return (
      <ChatMessageShell
        role="assistant"
        actorId={agentId}
        actorName={agentName}
        createdAt={item.createdAt}
      >
        <AssistantMessage
          taskId={item.taskId}
          isPending={isPending}
          transformContent={transformContent}
          onQuickAction={onQuickAction}
          quickActionsDisabled={quickActionsDisabled}
        />
      </ChatMessageShell>
    );
  }

  const { message } = item;

  if (message.role === "user") {
    return (
      <ChatMessageShell
        role="user"
        actorType={messageActor?.actorType ?? "member"}
        actorId={messageActor ? messageActor.actorId : userId}
        actorName={messageActor ? messageActor.actorName : userName}
        createdAt={message.created_at}
      >
        <div className="rounded-2xl bg-muted px-3.5 py-2 text-body max-w-[80%] break-words">
          {/* User messages are authored as markdown in ContentEditor, so they
           * render through the SAME RichContent as assistant replies and as
           * Issue/Comment — a Mermaid fence a user pastes is a diagram here
           * too. `compact` trims the leading/trailing block margins so a
           * single-line bubble stays as tight as the plain-text version. */}
          <RichContent
            content={message.content}
            attachments={message.attachments}
            density="compact"
            phase="settled"
          />
          <AttachmentList
            attachments={message.attachments}
            content={message.content}
            className="mt-1.5"
          />
        </div>
      </ChatMessageShell>
    );
  }

  return (
    <ChatMessageShell
      role="assistant"
      actorId={agentId}
      actorName={agentName}
      createdAt={message.created_at}
    >
      <AssistantMessage
        taskId={message.task_id ?? null}
        message={message}
        isPending={isPending}
        transformContent={transformContent}
        onQuickAction={onQuickAction}
        quickActionsDisabled={quickActionsDisabled}
        onRegenerateQuickActions={onRegenerateQuickActions}
        canRegenerateQuickActions={message.id === latestAssistantMessageId}
        quickActionsPending={quickActionsPendingMessageId === message.id}
        showStarterCards={message.id === starterCardsMessageId}
      />
    </ChatMessageShell>
  );
});

/**
 * LobeHub-style message geometry: identity and time form a light header, the
 * member reply stays a compact right-aligned bubble, and the assistant reply
 * occupies the full document column below its avatar rather than a card.
 */
function ChatMessageShell({
  role,
  actorType,
  actorId,
  actorName,
  createdAt,
  children,
}: {
  role: "user" | "assistant";
  actorType?: "member" | "agent";
  actorId?: string | null;
  actorName?: string | null;
  createdAt?: string;
  children: ReactNode;
}) {
  const locale = useLocale();
  const isUser = role === "user";
  const time = formatMessageTime(createdAt, locale);
  // A timestamp-only header is hidden until hover, so without an identity it
  // reserves an unreachable blank row for touch and keyboard users.
  const showHeader = !!actorId || !!actorName;

  return (
    <article
      className={cn(
        "group/message flex w-full flex-col gap-2",
        isUser ? "items-end pl-9" : "items-start",
      )}
    >
      {showHeader && (
        <header
          className={cn(
            "flex min-h-6 items-center gap-2",
            isUser && "flex-row-reverse",
          )}
        >
          {actorId && (
            <ActorAvatar
              actorType={actorType ?? (isUser ? "member" : "agent")}
              actorId={actorId}
              size="md"
              enableHoverCard
            />
          )}
          {actorName && (
            <span className="text-caption font-medium text-foreground">
              {actorName}
            </span>
          )}
          {time && (
            <time
              dateTime={createdAt}
              className="text-caption text-muted-foreground opacity-0 transition-opacity group-hover/message:opacity-100 group-focus-within/message:opacity-100"
            >
              {time}
            </time>
          )}
        </header>
      )}
      <div
        className={cn(
          "w-full max-w-full overflow-hidden",
          isUser && "flex justify-end",
        )}
      >
        {children}
      </div>
    </article>
  );
}

/**
 * Assistant turn body — renders BOTH the in-flight (live) and the persisted
 * form of one task (PB-4922).
 *
 * `message` is undefined while the task streams and becomes the persisted
 * `chat_message` when it lands. Both forms are rendered by this one component,
 * mounted under one stable row key (`task:<taskId>`), so the live → persisted
 * handoff is a prop change rather than an unmount: the RichContent subtree and
 * any Mermaid diagram / HTML iframe inside it stay mounted, keep their pan-zoom
 * state, and never re-run their expensive render. Before this, the live
 * timeline lived in Virtuoso's Footer and the persisted row keyed on
 * `message.id`, so every completed task tore down and rebuilt its diagrams.
 *
 * The timeline itself comes from `taskMessagesOptions(taskId)` in both forms —
 * the same cache entry useRealtimeSync seeds during execution — so no refetch
 * and no data discontinuity happens at the handoff either.
 */
function AssistantMessage({
  taskId,
  message,
  isPending,
  transformContent,
  onQuickAction,
  quickActionsDisabled,
  onRegenerateQuickActions,
  canRegenerateQuickActions = false,
  quickActionsPending = false,
  showStarterCards = false,
}: {
  taskId: string | null;
  message?: ChatMessage;
  isPending: boolean;
  transformContent?: (content: string) => string;
  onQuickAction?: (action: ChatQuickAction) => void | Promise<unknown>;
  quickActionsDisabled: boolean;
  onRegenerateQuickActions?: (message: ChatMessage) => void | Promise<unknown>;
  canRegenerateQuickActions?: boolean;
  quickActionsPending?: boolean;
  /** This turn is Mika's onboarding opening — render starter cards, not chips. */
  showStarterCards?: boolean;
}) {
  const canFetchTaskMessages = isTaskMessageTaskId(taskId);

  // Use the shared taskMessagesOptions so this cache entry is the same one
  // seeded by useRealtimeSync during task execution — zero refetch when the
  // task finishes, since WS already populated it.
  const { data: taskMessages } = useQuery({
    ...taskMessagesOptions(taskId ?? ""),
    enabled: canFetchTaskMessages,
  });

  // Memoized on the cache array identity: mergeTaskMessagesBySeq preserves the
  // array reference when a duplicate event arrives, so this recomputes only
  // when a genuinely new message lands.
  const timeline: ChatTimelineItem[] = useMemo(
    () => transformTimeline(buildTimeline(taskMessages ?? []), transformContent),
    [taskMessages, transformContent],
  );
  const visibleTimeline = getVisibleTimelineBlocks(timeline);
  const hasVisibleTimeline = visibleTimeline.length > 0;
  const hasVisibleText = visibleTimeline.some((block) => block.type === "text");

  // Content is settled once the persisted message exists; until then text is
  // still arriving and a trailing fence may be half-written.
  const phase: "streaming" | "settled" = message ? "settled" : "streaming";

  // Failure bubble path: when the server's FailTask wrote a failure
  // chat_message (failure_reason set), render a destructive bubble with the
  // human-readable reason label + collapsible raw errMsg + the same timeline
  // so the user can see exactly where the run broke.
  if (message?.failure_reason) {
    return (
      <FailureBubble
        reason={message.failure_reason}
        rawError={message.content}
        timeline={timeline}
        elapsedMs={message.elapsed_ms}
      />
    );
  }

  // no_response path (PB-4351): the agent completed this direct-chat turn
  // without any text. Keep whatever public event timeline the run produced and
  // show a localized "no text reply" notice instead of an empty markdown block.
  const isNoResponse = message?.message_kind === "no_response";

  return (
    <div className="w-full space-y-1.5">
      {hasVisibleTimeline && (
        <TimelineView
          items={timeline}
          attachments={message?.attachments}
          phase={phase}
        />
      )}
      {isNoResponse ? (
        <NoResponseNotice />
      ) : message && !hasVisibleText ? (
        <RichContent
          content={message.content}
          attachments={message.attachments}
          density="compact"
          phase="settled"
          className="leading-relaxed"
        />
      ) : null}
      {message && (
        <>
          <AttachmentList
            attachments={message.attachments}
            content={message.content}
          />
          <MessageFooter
            message={message}
            timeline={timeline}
            isPending={isPending}
          />
          {onQuickAction && showStarterCards ? (
            // The opening's starter cards own this turn's suggestion strip
            // (PB-5765); the server skips chip generation for it.
            <OnboardingStarterCards
              onPick={onQuickAction}
              disabled={quickActionsDisabled || isPending}
            />
          ) : onQuickAction && (message.quick_actions?.length ?? 0) > 0 ? (
            <QuickActions
              actions={message.quick_actions ?? []}
              disabled={quickActionsDisabled || isPending}
              onSelect={onQuickAction}
              onRegenerate={
                onRegenerateQuickActions && canRegenerateQuickActions
                  ? () => onRegenerateQuickActions(message)
                  : undefined
              }
              pending={quickActionsPending}
            />
          ) : onQuickAction && quickActionsPending ? (
            <QuickActionsSkeleton />
          ) : null}
        </>
      )}
    </div>
  );
}

function transformTimeline(
  timeline: ChatTimelineItem[],
  transformContent?: (content: string) => string,
): ChatTimelineItem[] {
  return timeline.map((item) =>
    item.type === "text" && item.content
      ? {
          ...item,
          content: transformContent
            ? transformContent(stripChatQuickActionsProtocol(item.content))
            : stripChatQuickActionsProtocol(item.content),
        }
      : item,
  );
}

function QuickActions({
  actions,
  disabled,
  onSelect,
  onRegenerate,
  pending = false,
}: {
  actions: ChatQuickAction[];
  disabled: boolean;
  onSelect: (action: ChatQuickAction) => void | Promise<unknown>;
  /** Present only on the session's latest turn — re-runs the suggestion pass. */
  onRegenerate?: () => void | Promise<unknown>;
  /**
   * The turn is awaiting a supplement (a refresh is in flight): its old pills
   * stay visible but inert, and the refresh icon spins until chat:quick_actions
   * lands. Distinct from the local `regenerating` guard, which only covers the
   * click → HTTP-ack window before the pending marker is observed.
   */
  pending?: boolean;
}) {
  const { t } = useT("chat");
  const [submitting, setSubmitting] = useState(false);
  const [regenerating, setRegenerating] = useState(false);
  // The pending marker is the single source of truth: chat:quick_actions clears
  // it on success, and useQuickActionsPendingTimeout clears it from the query
  // cache if no supplement ever arrives. So `pending` going false is what stops
  // the spinner — no component-local "expired" flag that only masks the UI while
  // the cache stays stuck (PB-5149 review).
  const blocked = disabled || submitting || regenerating || pending;

  const handleSelect = async (action: ChatQuickAction) => {
    if (blocked) return;
    setSubmitting(true);
    try {
      await onSelect(action);
    } catch {
      // The send path owns user-facing error feedback and optimistic rollback.
      // Re-enable the chip so a transient failure can be retried.
    } finally {
      setSubmitting(false);
    }
  };

  const handleRegenerate = async () => {
    if (blocked || !onRegenerate) return;
    setRegenerating(true);
    try {
      await onRegenerate();
    } catch {
      // The caller's mutation rolls the pending marker back; surface a toast so
      // the silent re-enable isn't mistaken for "no suggestions this time".
      toast.error(t(($) => $.message_list.quick_actions_regenerate_failed));
    } finally {
      setRegenerating(false);
    }
  };

  const regenerateLabel = t(($) => $.message_list.quick_actions_regenerate);

  return (
    <div className="mt-2 border-t border-border/40 pt-2 animate-in fade-in slide-in-from-bottom-1 duration-300">
      <div className="flex flex-wrap items-center gap-2" aria-label="Suggested follow-ups">
        <QuickActionsHeading />
        {actions.slice(0, 3).map((action, index) => (
          // The whole pill previews its hidden prompt on hover: clicking
          // sends a message the user has never seen, in their name — the
          // tooltip flips that from commit-then-learn to learn-then-commit.
          <Tooltip key={`${action.label}-${index}`}>
            <TooltipTrigger
              render={
                <Button
                  type="button"
                  variant={action.primary ? "brandSubtle" : "outline"}
                  size="sm"
                  className="max-w-full rounded-full px-3"
                  disabled={blocked}
                  onClick={() => void handleSelect(action)}
                />
              }
            >
              <span className="truncate">{action.label}</span>
              {action.primary ? <ArrowUpRight aria-hidden="true" /> : null}
            </TooltipTrigger>
            <TooltipContent side="top" className="max-w-sm whitespace-pre-wrap break-words">
              {action.prompt}
            </TooltipContent>
          </Tooltip>
        ))}
        {onRegenerate ? (
          <Tooltip>
            <TooltipTrigger
              render={
                <Button
                  type="button"
                  variant="ghost"
                  size="icon-xs"
                  className="shrink-0 rounded-full text-faint-foreground hover:text-foreground"
                  disabled={blocked}
                  aria-label={regenerateLabel}
                  onClick={() => void handleRegenerate()}
                />
              }
            >
              <RotateCw
                aria-hidden="true"
                className={
                  pending || regenerating ? "animate-spin" : undefined
                }
              />
            </TooltipTrigger>
            <TooltipContent side="top">{regenerateLabel}</TooltipContent>
          </Tooltip>
        ) : null}
      </div>
    </div>
  );
}

// Light inline prefix label for the follow-up pill row — the row sits below
// the reply footer ("Replied in Xs · Copy") behind a faint top border, so
// the pills read as a labelled next-steps strip, not part of the reply body.
// shrink-0 keeps the label whole at the row start when narrow widths wrap
// the pills.
function QuickActionsHeading() {
  const { t } = useT("chat");
  return (
    <span className="shrink-0 text-caption text-muted-foreground">
      {t(($) => $.message_list.quick_actions_heading)}
    </span>
  );
}

// Pill-shaped placeholders shown between chat:done (which declared a pending
// supplement) and chat:quick_actions. Widths are staggered so the row reads
// as "buttons coming", not a loading bar. aria-hidden: nothing actionable to
// announce yet.
function QuickActionsSkeleton() {
  // No local timeout: the shared pending marker drives visibility, and
  // useQuickActionsPendingTimeout clears it from the query cache if no
  // chat:quick_actions ever resolves it — so this unmounts on its own instead
  // of only hiding itself while the cache stays stuck (PB-5149 review).
  return (
    <div className="mt-2 border-t border-border/40 pt-2 animate-in fade-in duration-300">
      <div className="flex flex-wrap items-center gap-2" aria-hidden="true">
        <QuickActionsHeading />
        <Skeleton className="h-8 w-24 rounded-full" />
        <Skeleton className="h-8 w-32 rounded-full" />
        <Skeleton className="h-8 w-28 rounded-full" />
      </div>
    </div>
  );
}

// Muted, localized notice shown in place of assistant text when a turn
// completed with no reply (message_kind === "no_response"). Explains the empty
// turn instead of rendering a blank bubble (PB-4351).
function NoResponseNotice() {
  const { t } = useT("chat");
  return (
    <div className="text-body italic text-muted-foreground">
      {t(($) => $.message_list.no_response)}
    </div>
  );
}

// Inline footer row beneath the assistant reply: "Replied in 38s · [Copy]".
// Action icons live here (not as a hover-floating overlay) so they're
// discoverable on first read and don't shift content. Buttons stay quiet
// (muted) until hover. Copy is suppressed during streaming because the
// final text is still being appended.
function MessageFooter({
  message,
  timeline,
  isPending,
}: {
  message: ChatMessage;
  timeline: ChatTimelineItem[];
  isPending: boolean;
}) {
  // A no_response turn has nothing to copy, and its caption uses a neutral
  // "Finished in Xs" instead of "Replied in Xs" (PB-4351).
  const isNoResponse = message.message_kind === "no_response";
  const showCopy = !isPending && !isNoResponse;
  if (message.elapsed_ms == null && !showCopy) return null;
  return (
    <div className="flex items-center gap-1.5">
      {message.elapsed_ms != null && (
        <ElapsedCaption
          variant={isNoResponse ? "finished" : "replied"}
          elapsedMs={message.elapsed_ms}
        />
      )}
      {showCopy && <MessageCopyButton message={message} timeline={timeline} />}
    </div>
  );
}

function MessageCopyButton({
  message,
  timeline,
}: {
  message: ChatMessage;
  timeline: ChatTimelineItem[];
}) {
  const { t } = useT("chat");
  const handleCopy = async () => {
    if (await copyText(extractCopyText(message, timeline))) {
      toast.success(t(($) => $.message_list.copied_toast));
    } else {
      toast.error(t(($) => $.message_list.copy_failed_toast));
    }
  };
  return (
    <Tooltip>
      <TooltipTrigger
        render={
          <Button
            variant="ghost"
            size="icon-xs"
            className="text-faint-foreground hover:text-foreground"
            onClick={handleCopy}
            aria-label={t(($) => $.message_list.copy_action)}
          />
        }
      >
        <Copy />
      </TooltipTrigger>
      <TooltipContent side="top">
        {t(($) => $.message_list.copy_action)}
      </TooltipContent>
    </Tooltip>
  );
}

// Persisted "Replied in 38s" / "Failed after 12s" line under the assistant
// bubble. Reads `elapsed_ms` straight off the chat_message — server computes
// it once at task completion, so this caption is identical across reloads
// and devices. Skipped silently when null (legacy messages predating
// migration 063 + user messages).
function ElapsedCaption({
  variant,
  elapsedMs,
  className,
}: {
  variant: "replied" | "failed" | "finished";
  elapsedMs: number;
  className?: string;
}) {
  const { t } = useT("chat");
  const elapsed = formatElapsedMs(elapsedMs);
  const text =
    variant === "replied"
      ? t(($) => $.message_list.replied_in, { elapsed })
      : variant === "finished"
        ? t(($) => $.message_list.finished_in, { elapsed })
        : t(($) => $.message_list.failed_after, { elapsed });
  return (
    <div className={cn("text-caption text-muted-foreground", className)}>
      {text}
    </div>
  );
}

function FailureBubble({
  reason,
  rawError,
  timeline,
  elapsedMs,
}: {
  reason: string;
  rawError: string;
  timeline: ChatTimelineItem[];
  elapsedMs?: number | null;
}) {
  const { t } = useT("chat");
  const [open, setOpen] = useState(false);
  // Chat gets its own friendly, reassuring copy per failure reason — plain
  // language + a "try again" nudge — instead of the terse developer labels
  // (`failureReasonLabel`) used on the Agent thread surface.
  // The raw error stays tucked under the collapsible below for anyone who
  // wants the technical detail.
  //
  // Keyed by the raw wire value, not a closed enum — `failure_reason` is an
  // open string that grows as classifier rules land, same as
  // `failureReasonLabel`'s map. Deliberately partial: the taxonomy is larger
  // than the set worth writing distinct chat copy for, so an entry earns its
  // place only when it can say something the `agent_error` family line can't,
  // usually a different next step (re-auth, top up, check the network).
  //
  // Where this diverges from the operator surfaces: they fall back to the raw
  // wire value, which is machine-y but searchable. A chat bubble is read by
  // the person who just sent a message, so it degrades through
  // `resolveFailureReasonKey` to the family line and finally to friendly
  // generic copy. The raw error is still one click away under the collapsible.
  const chatFailureCopy: Record<string, string> = {
    agent_error: t(($) => $.message_list.failure.agent_error),
    timeout: t(($) => $.message_list.failure.timeout),
    codex_semantic_inactivity: t(($) => $.message_list.failure.codex_semantic_inactivity),
    runtime_offline: t(($) => $.message_list.failure.runtime_offline),
    runtime_recovery: t(($) => $.message_list.failure.runtime_recovery),
    manual: t(($) => $.message_list.failure.manual),
    cancelled: t(($) => $.message_list.failure.manual),
    skill_bundle_unavailable: t(($) => $.message_list.failure.skill_bundle_unavailable),
    runtime_cli_timeout: t(($) => $.message_list.failure.runtime_cli_timeout),
    "agent_error.provider_network": t(($) => $.message_list.failure.provider_network),
    "agent_error.provider_auth_or_access": t(($) => $.message_list.failure.provider_auth_or_access),
    "agent_error.provider_quota_limit": t(($) => $.message_list.failure.provider_quota_limit),
    "agent_error.provider_capacity_or_rate_limit": t(
      ($) => $.message_list.failure.provider_capacity_or_rate_limit,
    ),
    "agent_error.context_overflow": t(($) => $.message_list.failure.context_overflow),
    "agent_error.runtime_missing_executable": t(
      ($) => $.message_list.failure.runtime_missing_executable,
    ),
    "agent_error.runtime_version_unsupported": t(
      ($) => $.message_list.failure.runtime_version_unsupported,
    ),
  };
  const copyKey = resolveFailureReasonKey(reason, chatFailureCopy);
  const label =
    (copyKey && chatFailureCopy[copyKey]) ??
    t(($) => $.message_list.failure.fallback);

  return (
    <div className="w-full space-y-1.5">
      {/* Failure read as an inline, low-key note — not a destructive
       *  alert. Intentionally borderless / no background tint: a chat
       *  failure is informational ("this didn't work"), not a system
       *  error. The icon + muted destructive text are signal enough,
       *  the rest stays in the normal reply rhythm. */}
      <div className="flex items-start gap-1.5 text-body">
        <AlertTriangle className="size-3.5 shrink-0 text-destructive mt-0.5" />
        <div className="flex-1 min-w-0">
          <div className="text-destructive">{label}</div>
          {rawError.trim() && (
            <Collapsible open={open} onOpenChange={setOpen}>
              <CollapsibleTrigger className="mt-0.5 flex items-center gap-1 text-caption text-muted-foreground hover:text-foreground transition-colors">
                {open ? (
                  <ChevronDown className="size-3" />
                ) : (
                  <ChevronRight className="size-3" />
                )}
                <span>{t(($) => $.message_list.show_details)}</span>
              </CollapsibleTrigger>
              <CollapsibleContent>
                <pre className="mt-1 max-h-40 overflow-auto rounded bg-muted/40 p-2 text-caption text-muted-foreground whitespace-pre-wrap break-all">
                  {rawError}
                </pre>
              </CollapsibleContent>
            </Collapsible>
          )}
        </div>
      </div>
      {timeline.length > 0 && <TimelineView items={timeline} />}
      {elapsedMs != null && (
        <ElapsedCaption variant="failed" elapsedMs={elapsedMs} />
      )}
    </div>
  );
}

// ─── Timeline: document text + public execution events ───────────────────

type VisibleTimelineBlock =
  | {
      seq: number;
      type: "text" | "error";
      content: string;
    }
  | {
      seq: number;
      type: "tool_use" | "tool_result";
      event: ChatTimelineItem;
    };

/**
 * Project the task event stream into the public Agent thread contract.
 * Provider thinking is intentionally retained in the cache for audit and
 * status decisions, but its content is never a user-visible message. Tool
 * calls/results and errors are compact structured rows; final agent text stays
 * in the normal document flow.
 */
function getVisibleTimelineBlocks(
  items: ChatTimelineItem[],
): VisibleTimelineBlock[] {
  const blocks: VisibleTimelineBlock[] = [];

  for (const item of items) {
    // Internal provider reasoning must never become a disclosure, copied text,
    // or DOM content. TaskStatusPill still uses its type as a generic state.
    if (item.type === "thinking") {
      continue;
    }

    if (item.type === "tool_use" || item.type === "tool_result") {
      blocks.push({ seq: item.seq, type: item.type, event: item });
      continue;
    }

    if (item.type !== "text" && item.type !== "error") continue;

    const content = item.content?.trim() ? item.content : "";
    if (!content) continue;

    const previous = blocks.at(-1);
    if (
      previous &&
      (previous.type === "text" || previous.type === "error") &&
      item.type !== "error" &&
      previous.type === item.type
    ) {
      blocks[blocks.length - 1] = {
        ...previous,
        content: `${previous.content}${content}`,
      };
    } else {
      blocks.push({ seq: item.seq, type: item.type, content });
    }
  }

  return blocks;
}

function TimelineView({
  items,
  attachments,
  phase = "settled",
}: {
  items: ChatTimelineItem[];
  attachments?: import("@patchbay/core/types").Attachment[];
  phase?: "streaming" | "settled";
}) {
  const blocks = getVisibleTimelineBlocks(items);

  return (
    <div className="space-y-3">
      {blocks.map((block) => {
        if (block.type === "tool_use" || block.type === "tool_result") {
          return <ToolEventRow key={`${block.type}:${block.seq}`} event={block.event} />;
        }
        if (block.type === "error") {
          return <ErrorRow key={`error:${block.seq}`} content={block.content} />;
        }
        return (
          <RichContent
            key={`text:${block.seq}`}
            content={block.content}
            attachments={attachments}
            density="compact"
            phase={phase}
            className="leading-relaxed"
          />
        );
      })}
    </div>
  );
}

function ToolEventRow({ event }: { event: ChatTimelineItem }) {
  const { t } = useT("chat");
  const isResult = event.type === "tool_result";
  const tool = event.tool?.trim();
  const label = isResult
    ? tool
      ? t(($) => $.message_list.tool_result_named, { tool })
      : t(($) => $.message_list.tool_result_unnamed)
    : tool || t(($) => $.message_list.tool_fallback);
  const summary = redactSecrets(traceEventSummary(event)).trim();

  return (
    <div
      data-agent-thread-event={event.type}
      data-testid={`agent-thread-event-${event.type}`}
      className="flex min-w-0 items-baseline gap-2 rounded-md border border-border/50 bg-muted/20 px-2.5 py-1.5 text-caption text-muted-foreground"
    >
      <span className="shrink-0 font-medium text-foreground">{label}</span>
      {summary ? <span className="min-w-0 truncate">{summary}</span> : null}
    </div>
  );
}

function ErrorRow({ content }: { content: string }) {
  return (
    <div className="flex items-start gap-1.5 py-0.5 text-caption">
      <AlertCircle className="h-3 w-3 shrink-0 text-destructive mt-0.5" />
      <span className="text-destructive">{content}</span>
    </div>
  );
}

// ─── Shared ──────────────────────────────────────────────────────────────
