"use client";

import {
  memo,
  useCallback,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
} from "react";
import { toast } from "sonner";
import { useQuery } from "@tanstack/react-query";
import { Virtuoso, type Components, type VirtuosoHandle } from "react-virtuoso";
import { cn } from "@orvilo/ui/lib/utils";
import { Skeleton } from "@orvilo/ui/components/ui/skeleton";
import { Button } from "@orvilo/ui/components/ui/button";
import {
  Message as AIMessage,
  MessageActions as AIMessageActions,
  MessageContent as AIMessageContent,
} from "@orvilo/ui/components/ai-elements/message";
import {
  Suggestion as AISuggestion,
  Suggestions as AISuggestions,
} from "@orvilo/ui/components/ai-elements/suggestion";
import {
  ChainOfThought,
  ChainOfThoughtContent,
  ChainOfThoughtHeader,
  ChainOfThoughtStep,
} from "@orvilo/ui/components/ai-elements/chain-of-thought";
import {
  Reasoning,
  ReasoningContent,
  ReasoningTrigger,
} from "@orvilo/ui/components/ai-elements/reasoning";
import {
  Tool,
  ToolContent,
  ToolHeader,
  ToolInput,
  ToolOutput,
} from "@orvilo/ui/components/ai-elements/tool";
import { Conversation } from "@orvilo/ui/components/ai-elements/conversation";
import { Attachments as AIAttachments } from "@orvilo/ui/components/ai-elements/attachments";
import { Shimmer } from "@orvilo/ui/components/ai-elements/shimmer";
import {
  Context as AIContext,
  ContextContent,
  ContextContentBody,
  ContextContentHeader,
  ContextTrigger,
} from "@orvilo/ui/components/ai-elements/context";
import {
  Source,
  Sources,
  SourcesContent,
  SourcesTrigger,
} from "@orvilo/ui/components/ai-elements/sources";
import {
  InlineCitation,
  InlineCitationCard,
  InlineCitationCardBody,
  InlineCitationCardTrigger,
  InlineCitationQuote,
  InlineCitationSource,
  InlineCitationText,
} from "@orvilo/ui/components/ai-elements/inline-citation";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@orvilo/ui/components/ui/collapsible";
import {
  Tooltip,
  TooltipTrigger,
  TooltipContent,
} from "@orvilo/ui/components/ui/tooltip";
import {
  ChevronRight,
  ChevronDown,
  AlertCircle,
  AlertTriangle,
  ArrowUpRight,
  Copy,
  RotateCw,
} from "lucide-react";
import { useScrollFade } from "@orvilo/ui/hooks/use-scroll-fade";
import {
  isTaskMessageTaskId,
  taskMessagesOptions,
} from "@orvilo/core/chat/queries";
import { RichContent } from "../../rich-content";
import { RichContentScrollRootProvider } from "../../rich-content/scroll-root";
import { copyText } from "@orvilo/ui/lib/clipboard";
import { AttachmentList } from "../../issues/components/comment-card";
import { ImageSequenceProvider } from "../../editor";
import { collectImageSequence } from "@orvilo/core/attachments/image-sequence";
import type { AgentAvailability } from "@orvilo/core/agents";
import { resolveFailureReasonKey } from "@orvilo/core/agents";
import type {
  ChatMessage,
  ChatPendingTask,
  ChatQuickAction,
  TaskMessagePayload,
} from "@orvilo/core/types";
import type { ChatTimelineItem } from "@orvilo/core/chat";
import { buildTimeline } from "../../common/task-transcript/build-timeline";
import { OnboardingStarterCards } from "./onboarding-starter-cards";
import { TaskStatusPill } from "./task-status-pill";
import { CHAT_COLUMN, CHAT_GUTTER } from "./chat-column";
import { FOLLOW_EDGE_THRESHOLD } from "../../common/task-transcript/transcript-follow";
import { LIVE_END_ROW_ATTR, useStickToBottom } from "./stick-to-bottom";
import { formatElapsedMs } from "../lib/format";
import { splitTimeline, extractCopyText } from "../lib/copy-text";
import { stripChatQuickActionsProtocol } from "../lib/quick-actions";
import { useT } from "../../i18n";

// ─── Public component ────────────────────────────────────────────────────

interface ChatMessageListProps {
  messages: ChatMessage[];
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

  onRegenerateQuickActions?: (message: ChatMessage) => void | Promise<unknown>;
  /**
   * Message currently awaiting its quick-actions supplement (client-only
   * marker raised by chat:done or a refresh) — renders pill skeletons under
   * that reply until chat:quick_actions resolves it.
   */
  quickActionsPendingMessageId?: string | null;
  /** Hide provider thinking/tool rows on compact embedded conversation surfaces. */
  showProcessSteps?: boolean;
}

// ─── Virtuoso chrome ─────────────────────────────────────────────────────
//
// Header/Footer MUST be stable component references (module scope), never
// inline arrows in the `components` prop: an inline `components={{ Footer:
// () => … }}` creates a new component *type* every render, so React unmounts
// and remounts the whole Header/Footer subtree each time. During task
// streaming that tore down and rebuilt the entire live timeline — every row
// and every Markdown parse — on every `task:message` event, freezing the

// Virtuoso's `context` prop instead, which reaches these components as an
// ordinary prop (re-render, not remount).

interface ChatListContext {
  isFetchingOlderMessages: boolean;
  showStatusPill: boolean;
  pendingTask: ChatPendingTask | null | undefined;
  liveTaskMessages: readonly TaskMessagePayload[] | undefined;
  availability: AgentAvailability | undefined;
}

type ChatRenderItem =
  | {
      key: string;
      kind: "message";
      message: ChatMessage;
      taskId: string | null;
    }
  | { key: string; kind: "live"; taskId: string };

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
  showProcessSteps = true,
}: ChatMessageListProps) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const [scrollContainerEl, setScrollContainerEl] =
    useState<HTMLDivElement | null>(null);
  const setScrollContainerRef = useCallback((node: HTMLDivElement | null) => {
    scrollRef.current = node;
    setScrollContainerEl(node);
  }, []);
  const virtuosoRef = useRef<VirtuosoHandle>(null);
  // The bottom-stick corrects through Virtuoso, never by writing `scrollTop`
  // on the container: `scrollHeight` is an estimate over the unrendered rows,
  // so the pixel bottom moves as Virtuoso measures (see stick-to-bottom.ts).
  const pinToLiveEnd = useCallback(() => {
    virtuosoRef.current?.scrollToIndex({ index: "LAST", align: "end" });
  }, []);
  const { isFollowing, onContentHeightChanged, hasReachedLiveEnd } =
    useStickToBottom(scrollContainerEl, pinToLiveEnd);
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

  // Patrick's onboarding opening self-describes (message_kind stamped by the
  // completion path — the hidden kickoff row never reaches clients) and
  // carries the product's starter cards instead of that turn's quick-action

  const starterCardsMessageId = useMemo(
    () =>
      messages.find(
        (m) =>
          m.role === "assistant" && m.message_kind === "onboarding_opening",
      )?.id ?? null,
    [messages],
  );

  // Once the assistant message for this pending task has landed in the
  // messages list, AssistantMessage owns its rendering — suppress the live
  // timeline (and pill) to avoid rendering the same content in two places
  // during the invalidate → refetch window.
  const pendingAlreadyPersisted =
    !!pendingTaskId &&
    messages.some((m) => m.role === "assistant" && m.task_id === pendingTaskId);

  // Live timeline for the in-flight task. useRealtimeSync keeps this cache
  // current via setQueryData on task:message events. Only used here to decide
  // whether the live row exists and to feed the status pill — the row itself
  // reads the same cache entry through AssistantMessage.
  const showLiveTimeline = !!pendingTaskId && !pendingAlreadyPersisted;
  const canFetchLiveTimeline =
    isTaskMessageTaskId(pendingTaskId) && !pendingAlreadyPersisted;
  const { data: liveTaskMessages } = useQuery({
    ...taskMessagesOptions(pendingTaskId ?? ""),
    enabled: canFetchLiveTimeline,
  });
  const hasLive = showLiveTimeline && (liveTaskMessages?.length ?? 0) > 0;
  const showStatusPill =
    !!pendingTaskId && !pendingAlreadyPersisted && !!pendingTask;

  // Persisted messages plus, while a task is in flight, one synthetic trailing
  // row for it. When the assistant message persists, `hasLive` goes false and
  // the message takes the SAME key at the SAME position — an in-place data
  // swap, not a remount. The onboarding kickoff is a server-authored carrier
  // for Patrick's first task, not something the member typed, so it never becomes
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
      });
    }
    return items;
  }, [messages, hasLive, pendingTaskId]);

  const firstIndex = renderItems.length > 0 ? firstItemIndex : 0;
  const liveEndKey = renderItems[renderItems.length - 1]?.key ?? null;

  const listContext: ChatListContext = {
    isFetchingOlderMessages,
    showStatusPill,
    pendingTask,
    liveTaskMessages,
    availability,
  };

  // Every image in this session, in message order, so opening one lets the

  // from what Virtuoso currently has mounted.
  //
  // Persisted messages only: a task transcript's own attachments live behind a
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
      <Conversation managed={false} className="contents">
        <div
          ref={setScrollContainerRef}
          data-tab-scroll-root
          style={fadeStyle}
          // The gutter lives on the scroll container, so it applies once to the
          // whole list — rows, header, footer — and the scrollbar still rides the
          // surface edge rather than being inset with the text.
          // Hidden until Virtuoso has actually landed on the newest message. The
          // container paints nothing for that whole window anyway — the rows are
          // not measured yet — so this costs no visible time and spares the
          // reader a frame of the wrong messages (see stick-to-bottom.ts).
          className={cn(
            "flex-1 overflow-y-auto",
            CHAT_GUTTER,
            !hasReachedLiveEnd && "invisible",
          )}
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
                ref={virtuosoRef}
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
                atBottomThreshold={FOLLOW_EDGE_THRESHOLD}
                // Follow rapid streamed output only while Virtuoso says the reader is
                // at the live end. An in-flight smooth animation temporarily reports
                // "not at bottom" on the next append and permanently drops the follow
                // (#6697), so live growth must use an immediate scroll. `isFollowing`
                // narrows this further: the reader may have scrolled away by input the
                // 120px `atBottom` band forgives (see stick-to-bottom.ts).
                followOutput={(atBottom) =>
                  !isFetchingOlderMessages && atBottom && isFollowing()
                    ? "auto"
                    : false
                }
                // `followOutput` never fires for a single row growing mid-stream, so
                // content resizes route to the bottom-stick through Virtuoso's own
                // height signal instead.
                totalListHeightChanged={onContentHeightChanged}
                startReached={() => {
                  if (hasOlderMessages && !isFetchingOlderMessages) {
                    onLoadOlderMessages?.();
                  }
                }}
                computeItemKey={(_, item) => item.key}
                context={listContext}
                components={LIST_COMPONENTS}
                itemContent={(_, item) => (
                  <div
                    className={cn(CHAT_COLUMN, "py-2")}
                    {...(item.key === liveEndKey
                      ? { [LIVE_END_ROW_ATTR]: "" }
                      : {})}
                  >
                    <MessageBubble
                      item={item}
                      isPending={
                        !!pendingTaskId && item.taskId === pendingTaskId
                      }
                      transformContent={transformContent}
                      onQuickAction={onQuickAction}
                      quickActionsDisabled={quickActionsDisabled}
                      onRegenerateQuickActions={onRegenerateQuickActions}
                      latestAssistantMessageId={latestAssistantMessageId}
                      quickActionsPendingMessageId={
                        quickActionsPendingMessageId
                      }
                      starterCardsMessageId={starterCardsMessageId}
                      showProcessSteps={showProcessSteps}
                    />
                  </div>
                )}
              />
            </RichContentScrollRootProvider>
          )}
        </div>
      </Conversation>
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
// history stays inert while only the live footer updates.
const MessageBubble = memo(function MessageBubble({
  item,
  isPending,
  transformContent,
  onQuickAction,
  quickActionsDisabled,
  onRegenerateQuickActions,
  latestAssistantMessageId,
  quickActionsPendingMessageId,
  starterCardsMessageId,
  showProcessSteps,
}: {
  item: ChatRenderItem;
  isPending: boolean;
  transformContent?: (content: string) => string;
  onQuickAction?: (action: ChatQuickAction) => void | Promise<unknown>;
  quickActionsDisabled: boolean;
  onRegenerateQuickActions?: (message: ChatMessage) => void | Promise<unknown>;
  latestAssistantMessageId: string | null;
  quickActionsPendingMessageId: string | null;
  starterCardsMessageId: string | null;
  showProcessSteps: boolean;
}) {
  // The live row and the persisted assistant row both land here under one key,
  // and both render <AssistantMessage> — same component type, same position —
  // so React reconciles rather than remounts at task completion.
  if (item.kind === "live") {
    return (
      <AssistantMessage
        taskId={item.taskId}
        isPending={isPending}
        transformContent={transformContent}
        onQuickAction={onQuickAction}
        quickActionsDisabled={quickActionsDisabled}
        showProcessSteps={showProcessSteps}
      />
    );
  }

  const { message } = item;

  if (message.role === "user") {
    return (
      <AIMessage from="user" className="max-w-full">
        <AIMessageContent className="rounded-2xl bg-muted px-3.5 py-2 text-body max-w-[80%] break-words">
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
          <AIAttachments variant="list" className="mt-1.5">
            <AttachmentList
              attachments={message.attachments}
              content={message.content}
            />
          </AIAttachments>
        </AIMessageContent>
      </AIMessage>
    );
  }

  return (
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
      showProcessSteps={showProcessSteps}
    />
  );
});

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
  showProcessSteps = true,
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
  /** This turn is Patrick's onboarding opening — render starter cards, not chips. */
  showStarterCards?: boolean;
  showProcessSteps?: boolean;
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
    () =>
      transformTimeline(buildTimeline(taskMessages ?? []), transformContent),
    [taskMessages, transformContent],
  );

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
        showProcessSteps={showProcessSteps}
      />
    );
  }

  // without any text. Keep whatever tool/thinking timeline the run produced and
  // show a localized "no text reply" notice instead of an empty markdown block.
  const isNoResponse = message?.message_kind === "no_response";

  return (
    <AIMessage from="assistant" className="max-w-full">
      <AIMessageContent className="w-full space-y-1.5 overflow-visible">
        {timeline.length > 0 && (
          <TimelineView
            items={timeline}
            message={message}
            attachments={message?.attachments}
            phase={phase}
            isStreaming={!message}
            showProcessSteps={showProcessSteps}
          />
        )}
        {isNoResponse ? (
          <NoResponseNotice />
        ) : message && timeline.length === 0 ? (
          <AssistantRichContent content={message.content} message={message} />
        ) : null}
        {message && (
          <>
            <AIAttachments variant="list">
              <AttachmentList
                attachments={message.attachments}
                content={message.content}
              />
            </AIAttachments>
            <MessageSources message={message} />
            <MessageFooter
              message={message}
              timeline={timeline}
              isPending={isPending}
            />
            {onQuickAction && showStarterCards ? (
              // The opening's starter cards own this turn's suggestion strip

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
      </AIMessageContent>
    </AIMessage>
  );
}

function utf8Range(value: string, start: number, end: number): string {
  if (
    !Number.isInteger(start) ||
    !Number.isInteger(end) ||
    start < 0 ||
    end <= start
  ) {
    return "";
  }
  const bytes = new TextEncoder().encode(value);
  if (end > bytes.length) return "";
  return new TextDecoder("utf-8", { fatal: false })
    .decode(bytes.slice(start, end))
    .trim();
}

type ResolvedCitation = {
  source: NonNullable<ChatMessage["sources"]>[number];
  sourceNumber: number;
  quote: string;
  end: number;
};

function resolvedMessageCitations(message: ChatMessage): ResolvedCitation[] {
  const sources = (message.sources ?? []).filter((source) => {
    try {
      const url = new URL(source.url);
      return url.protocol === "https:" || url.protocol === "http:";
    } catch {
      return false;
    }
  });
  const byId = new Map(
    sources.map((source, index) => [
      source.id,
      { source, sourceNumber: index + 1 },
    ]),
  );
  return (message.citations ?? []).flatMap((citation) => {
    const resolved = byId.get(citation.source_id);
    const quote = utf8Range(message.content, citation.start, citation.end);
    return resolved && quote ? [{ ...resolved, quote, end: citation.end }] : [];
  });
}

function utf8OffsetToStringIndex(value: string, offset: number): number {
  return new TextDecoder("utf-8", { fatal: false }).decode(
    new TextEncoder().encode(value).slice(0, offset),
  ).length;
}

function CitationMarker({ citation }: { citation: ResolvedCitation }) {
  return (
    <InlineCitation>
      <InlineCitationText>{citation.sourceNumber}</InlineCitationText>
      <InlineCitationCard>
        <InlineCitationCardTrigger sources={[citation.source.url]} />
        <InlineCitationCardBody className="space-y-3 p-4">
          <InlineCitationSource
            title={citation.source.title ?? citation.source.url}
            url={citation.source.url}
          />
          <InlineCitationQuote>{citation.quote}</InlineCitationQuote>
        </InlineCitationCardBody>
      </InlineCitationCard>
    </InlineCitation>
  );
}

function AssistantRichContent({
  content,
  message,
  attachments,
  phase = "settled",
}: {
  content: string;
  message?: ChatMessage;
  attachments?: import("@orvilo/core/types").Attachment[];
  phase?: "streaming" | "settled";
}) {
  const markerScope = useId();
  const citations = useMemo(
    () => (message ? resolvedMessageCitations(message) : []),
    [message],
  );
  const inlineMarkers = useMemo(
    () =>
      citations.map((citation, index) => ({
        offset: utf8OffsetToStringIndex(content, citation.end),
        id: `${markerScope}/${index}`,
        label: String(citation.sourceNumber),
      })),
    [citations, markerScope, content],
  );
  const renderMarker = useCallback(
    (marker: string) => {
      const [scope, rawIndex] = marker.split("/");
      if (scope !== markerScope) return null;
      const index = Number(rawIndex);
      const citation = Number.isInteger(index) ? citations[index] : undefined;
      return citation ? <CitationMarker citation={citation} /> : null;
    },
    [citations, markerScope],
  );
  return (
    <RichContent
      content={content}
      attachments={message?.attachments ?? attachments}
      density="compact"
      phase={phase}
      className="leading-relaxed"
      inlineMarkerRenderer={renderMarker}
      inlineMarkers={inlineMarkers}
    />
  );
}

function MessageSources({ message }: { message: ChatMessage }) {
  const sources = (message.sources ?? []).filter((source) => {
    try {
      const url = new URL(source.url);
      return url.protocol === "https:" || url.protocol === "http:";
    } catch {
      return false;
    }
  });
  if (sources.length === 0) return null;

  return (
    <div className="space-y-2">
      <Sources>
        <SourcesTrigger count={sources.length} />
        <SourcesContent>
          {sources.map((source) => (
            <Source
              key={source.id}
              href={source.url}
              title={source.title ?? source.url}
            />
          ))}
        </SourcesContent>
      </Sources>
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
      <AISuggestions
        scrollable={false}
        className="flex-wrap whitespace-normal"
        aria-label={t(($) => $.message_list.quick_actions_aria)}
      >
        <QuickActionsHeading />
        {actions.slice(0, 3).map((action, index) => (
          // The whole pill previews its hidden prompt on hover: clicking
          // sends a message the user has never seen, in their name — the
          // tooltip flips that from commit-then-learn to learn-then-commit.
          <Tooltip key={`${action.label}-${index}`}>
            <TooltipTrigger
              render={
                <AISuggestion
                  suggestion={action.label}
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
            <TooltipContent
              side="top"
              className="max-w-sm whitespace-pre-wrap break-words"
            >
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
                className={pending || regenerating ? "animate-spin" : undefined}
              />
            </TooltipTrigger>
            <TooltipContent side="top">{regenerateLabel}</TooltipContent>
          </Tooltip>
        ) : null}
      </AISuggestions>
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
  const { t } = useT("chat");
  // No local timeout: the shared pending marker drives visibility, and
  // useQuickActionsPendingTimeout clears it from the query cache if no
  // chat:quick_actions ever resolves it — so this unmounts on its own instead

  return (
    <div className="mt-2 border-t border-border/40 pt-2 animate-in fade-in duration-300">
      <div className="flex flex-wrap items-center gap-2" aria-hidden="true">
        <Shimmer className="text-caption">
          {t(($) => $.message_list.quick_actions_heading)}
        </Shimmer>
        <Skeleton className="h-8 w-24 rounded-full" />
        <Skeleton className="h-8 w-32 rounded-full" />
        <Skeleton className="h-8 w-28 rounded-full" />
      </div>
    </div>
  );
}

// Muted, localized notice shown in place of assistant text when a turn
// completed with no reply (message_kind === "no_response"). Explains the empty

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

  const isNoResponse = message.message_kind === "no_response";
  const showCopy = !isPending && !isNoResponse;
  if (message.elapsed_ms == null && !showCopy) return null;
  return (
    <AIMessageActions className="gap-1.5">
      {message.elapsed_ms != null && (
        <ElapsedCaption
          variant={isNoResponse ? "finished" : "replied"}
          elapsedMs={message.elapsed_ms}
        />
      )}
      {message.usage?.length ? <MessageUsage usage={message.usage} /> : null}
      {showCopy && <MessageCopyButton message={message} timeline={timeline} />}
    </AIMessageActions>
  );
}

function MessageUsage({ usage }: { usage: NonNullable<ChatMessage["usage"]> }) {
  const { t } = useT("chat");
  const input = usage.reduce((sum, item) => sum + item.input_tokens, 0);
  const output = usage.reduce((sum, item) => sum + item.output_tokens, 0);
  const cacheRead = usage.reduce(
    (sum, item) => sum + item.cache_read_tokens,
    0,
  );
  const cacheWrite = usage.reduce(
    (sum, item) => sum + item.cache_write_tokens,
    0,
  );
  const total = input + output + cacheRead + cacheWrite;
  const formatter = new Intl.NumberFormat(undefined, { notation: "compact" });
  const model = usage.length === 1 ? usage[0]?.model : undefined;

  return (
    <AIContext usedTokens={total} modelId={model}>
      <ContextTrigger
        size="xs"
        className="h-6 gap-1 px-1.5 text-caption text-faint-foreground"
        aria-label={t(($) => $.message_list.token_usage_aria, { count: total })}
      >
        {t(($) => $.message_list.tokens, { count: formatter.format(total) })}
      </ContextTrigger>
      <ContextContent align="start">
        <ContextContentHeader />
        <ContextContentBody className="space-y-1 text-caption">
          <div className="flex justify-between gap-4">
            <span>{t(($) => $.message_list.input_tokens)}</span>
            <span>{input.toLocaleString()}</span>
          </div>
          <div className="flex justify-between gap-4">
            <span>{t(($) => $.message_list.output_tokens)}</span>
            <span>{output.toLocaleString()}</span>
          </div>
          {cacheRead > 0 ? (
            <div className="flex justify-between gap-4">
              <span>{t(($) => $.message_list.cache_read_tokens)}</span>
              <span>{cacheRead.toLocaleString()}</span>
            </div>
          ) : null}
          {cacheWrite > 0 ? (
            <div className="flex justify-between gap-4">
              <span>{t(($) => $.message_list.cache_write_tokens)}</span>
              <span>{cacheWrite.toLocaleString()}</span>
            </div>
          ) : null}
          {model ? (
            <div className="truncate border-t pt-1 text-muted-foreground">
              {model}
            </div>
          ) : null}
        </ContextContentBody>
      </ContextContent>
    </AIContext>
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
  showProcessSteps = true,
}: {
  reason: string;
  rawError: string;
  timeline: ChatTimelineItem[];
  elapsedMs?: number | null;
  showProcessSteps?: boolean;
}) {
  const { t } = useT("chat");
  const [open, setOpen] = useState(false);
  // Chat gets its own friendly, reassuring copy per failure reason — plain
  // language + a "try again" nudge — instead of the terse developer labels
  // (`failureReasonLabel`) used on the agent-detail / execution-log surfaces.
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
    codex_semantic_inactivity: t(
      ($) => $.message_list.failure.codex_semantic_inactivity,
    ),
    runtime_offline: t(($) => $.message_list.failure.runtime_offline),
    runtime_recovery: t(($) => $.message_list.failure.runtime_recovery),
    manual: t(($) => $.message_list.failure.manual),
    cancelled: t(($) => $.message_list.failure.manual),
    skill_bundle_unavailable: t(
      ($) => $.message_list.failure.skill_bundle_unavailable,
    ),
    runtime_cli_timeout: t(($) => $.message_list.failure.runtime_cli_timeout),
    "agent_error.provider_network": t(
      ($) => $.message_list.failure.provider_network,
    ),
    "agent_error.provider_auth_or_access": t(
      ($) => $.message_list.failure.provider_auth_or_access,
    ),
    "agent_error.provider_quota_limit": t(
      ($) => $.message_list.failure.provider_quota_limit,
    ),
    "agent_error.provider_capacity_or_rate_limit": t(
      ($) => $.message_list.failure.provider_capacity_or_rate_limit,
    ),
    "agent_error.context_overflow": t(
      ($) => $.message_list.failure.context_overflow,
    ),
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
      {timeline.length > 0 && (
        <TimelineView items={timeline} showProcessSteps={showProcessSteps} />
      )}
      {elapsedMs != null && (
        <ElapsedCaption variant="failed" elapsedMs={elapsedMs} />
      )}
    </div>
  );
}

// ─── Timeline: outer process fold + final text (Conductor-style) ─────────
//
// splitTimeline (lib/copy-text.ts) carves the items into:
//   preface — text before the first thinking/tool item
//   middle  — first → last non-text item (inclusive, may sandwich text)
//   final   — text after the last non-text item
//
// We render preface + final outside an outer Collapsible ("X steps") that
// wraps middle. The inner row Collapsibles (ThinkingRow / ToolCallRow /
// ToolResultRow) are unchanged — clicking them toggles independently of
// the outer fold. Copy mirrors what's visible when the outer fold is
// closed: preface + final, never middle. See extractCopyText for the
// authoritative copy logic.

function TimelineView({
  items,
  message,
  isStreaming,
  attachments,
  phase = "settled",
  showProcessSteps = true,
}: {
  items: ChatTimelineItem[];
  message?: ChatMessage;
  isStreaming?: boolean;
  attachments?: import("@orvilo/core/types").Attachment[];
  phase?: "streaming" | "settled";
  showProcessSteps?: boolean;
}) {
  const { preface, middle, final } = splitTimeline(items);
  const finalContent = message
    ? message.content
    : final.map((item) => item.content ?? "").join("");

  return (
    <>
      {!message && preface.length > 0 && (
        <RichContent
          content={preface.map((t) => t.content ?? "").join("")}
          attachments={attachments}
          density="compact"
          phase={phase}
          className="leading-relaxed"
        />
      )}
      {showProcessSteps && middle.length > 0 && (
        <OuterProcessFold
          items={middle}
          isStreaming={!!isStreaming}
          attachments={attachments}
          phase={phase}
        />
      )}
      {finalContent.length > 0 ? (
        <AssistantRichContent
          content={finalContent}
          message={message}
          attachments={attachments}
          phase={phase}
        />
      ) : null}
    </>
  );
}

function OuterProcessFold({
  items,
  isStreaming,
  attachments,
  phase = "settled",
}: {
  items: ChatTimelineItem[];
  isStreaming?: boolean;
  attachments?: import("@orvilo/core/types").Attachment[];
  phase?: "streaming" | "settled";
}) {
  const { t } = useT("chat");
  // Open while the task streams (so the user watches progress), collapsed once
  // it settles. This used to fall out of a remount: the live TimelineView was
  // torn down and the persisted one mounted closed. The row is now stable

  // HTML blocks alive — so the collapse has to be expressed directly.
  const [open, setOpen] = useState(!!isStreaming);
  const wasStreaming = useRef(!!isStreaming);
  useEffect(() => {
    if (wasStreaming.current && !isStreaming) setOpen(false);
    wasStreaming.current = !!isStreaming;
  }, [isStreaming]);
  const stepCount = items.length;

  return (
    <ChainOfThought open={open} onOpenChange={setOpen} className="space-y-0">
      <ChainOfThoughtHeader className="text-caption">
        {t(($) => $.message_list.process_steps, { count: stepCount })}
      </ChainOfThoughtHeader>
      <ChainOfThoughtContent className="mt-1 rounded-lg border bg-muted/20 p-2 space-y-0.5">
        {items.map((item) =>
          item.type === "text" ? (
            <MiddleTextRow
              key={item.seq}
              item={item}
              attachments={attachments}
              phase={phase}
            />
          ) : (
            <ChainOfThoughtStep
              key={item.seq}
              label={<ItemRow item={item} />}
              status={isStreaming ? "active" : "complete"}
              className="gap-1 [&>div:first-child]:hidden"
            />
          ),
        )}
      </ChainOfThoughtContent>
    </ChainOfThought>
  );
}

// Intermediate text segment rendered inside the outer fold. Visually
// down-shifted (xs / muted) so it reads as part of the agent's process,
// not the final answer — the final answer renders below the fold at full
// prose size.
function MiddleTextRow({
  item,
  attachments,
  phase = "settled",
}: {
  item: ChatTimelineItem;
  attachments?: import("@orvilo/core/types").Attachment[];
  phase?: "streaming" | "settled";
}) {
  return (
    <div className="py-0.5 text-caption text-muted-foreground">
      <RichContent
        content={item.content ?? ""}
        attachments={attachments}
        density="compact"
        phase={phase}
      />
    </div>
  );
}

// ─── Individual item rows ────────────────────────────────────────────────

function ItemRow({ item }: { item: ChatTimelineItem }) {
  switch (item.type) {
    case "tool_use":
      return <ToolCallRow item={item} />;
    case "tool_result":
      return <ToolResultRow item={item} />;
    case "thinking":
      return <ThinkingRow item={item} />;
    case "error":
      return <ErrorRow item={item} />;
    default:
      return null;
  }
}

function ToolCallRow({ item }: { item: ChatTimelineItem }) {
  const [open, setOpen] = useState(false);
  const hasInput = item.input && Object.keys(item.input).length > 0;
  const output = item.output ?? "";

  return (
    <Tool
      open={open}
      onOpenChange={setOpen}
      className="mb-0 border-0 bg-transparent"
    >
      <ToolHeader
        type="dynamic-tool"
        toolName={item.tool ?? "tool"}
        title={item.tool}
        state={item.state ?? "input-available"}
        className="justify-start gap-1.5 p-0.5 text-caption [&>div]:min-w-0 [&_.rounded-full]:hidden"
      />
      {hasInput || output ? (
        <ToolContent className="p-1">
          {hasInput ? <ToolInput input={item.input ?? {}} /> : null}
          {output ? (
            <ToolOutput
              output={
                item.state === "output-error"
                  ? undefined
                  : output.length > 4000
                    ? `${output.slice(0, 4000)}\n... (truncated)`
                    : output
              }
              errorText={item.state === "output-error" ? output : undefined}
            />
          ) : null}
        </ToolContent>
      ) : null}
    </Tool>
  );
}

function ToolResultRow({ item }: { item: ChatTimelineItem }) {
  const { t } = useT("chat");
  const [open, setOpen] = useState(false);
  const output = item.output ?? "";
  if (!output) return null;

  const preview = output.length > 120 ? output.slice(0, 120) + "..." : output;
  const labelPrefix = item.tool
    ? t(($) => $.message_list.tool_result_named, { tool: item.tool })
    : t(($) => $.message_list.tool_result_unnamed);

  return (
    <Tool
      open={open}
      onOpenChange={setOpen}
      className="mb-0 border-0 bg-transparent"
    >
      <ToolHeader
        type="dynamic-tool"
        toolName={item.tool ?? "tool"}
        title={`${labelPrefix}${preview}`}
        state={item.state ?? "output-available"}
        className="justify-start gap-1.5 p-0.5 text-caption [&>div]:min-w-0 [&_.rounded-full]:hidden"
      />
      <ToolContent className="p-1">
        <ToolOutput
          output={
            item.state === "output-error"
              ? undefined
              : output.length > 4000
                ? `${output.slice(0, 4000)}\n... (truncated)`
                : output
          }
          errorText={item.state === "output-error" ? output : undefined}
        />
      </ToolContent>
    </Tool>
  );
}

function ThinkingRow({ item }: { item: ChatTimelineItem }) {
  const [open, setOpen] = useState(false);
  const text = item.content ?? "";
  if (!text) return null;

  const preview = text.length > 150 ? text.slice(0, 150) + "..." : text;

  return (
    <Reasoning
      open={open}
      onOpenChange={setOpen}
      defaultOpen={false}
      className="mb-0"
    >
      <ReasoningTrigger
        className="gap-1.5 text-caption"
        getThinkingMessage={() => preview}
      />
      <ReasoningContent className="mt-1 max-h-40 overflow-auto text-caption">
        {text}
      </ReasoningContent>
    </Reasoning>
  );
}

function ErrorRow({ item }: { item: ChatTimelineItem }) {
  return (
    <div className="flex items-start gap-1.5 px-1 -mx-1 py-0.5 text-caption">
      <AlertCircle className="h-3 w-3 shrink-0 text-destructive mt-0.5" />
      <span className="text-destructive">{item.content}</span>
    </div>
  );
}

// ─── Shared ──────────────────────────────────────────────────────────────
