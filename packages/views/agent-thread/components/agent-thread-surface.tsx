"use client";

import { useId, type ComponentProps, type ReactNode } from "react";
import { ArrowLeft, PanelRightClose } from "lucide-react";
import type { AgentAvailability } from "@orvilo/core/agents";
import type {
  ChatMessage,
  ChatPendingTask,
  ChatQueuedTask,
} from "@orvilo/core/types";
import { Button } from "@orvilo/ui/components/ui/button";
import {
  Checkpoint,
  CheckpointIcon,
} from "@orvilo/ui/components/ai-elements/checkpoint";
import { ActorAvatar } from "../../common/actor-avatar";
import { ChatInput } from "../../chat/components/chat-input";
import { ChatQueue } from "../../chat/components/chat-queue";
import {
  ChatMessageList,
  ChatMessageSkeleton,
} from "../../chat/components/chat-message-list";

export type AgentThreadSubmit = ComponentProps<typeof ChatInput>["onSend"];

const blockedAgentThreadSubmit: AgentThreadSubmit = () => false;

export interface AgentThreadSurfaceProps {
  onClose: () => void;
  collapseLabel: string;
  agentId: string;
  agentName: string;
  title: ReactNode;
  description?: ReactNode;
  descriptionHint?: ReactNode;
  messages: ChatMessage[];
  pendingTask: ChatPendingTask | null | undefined;
  availability: AgentAvailability | undefined;
  isLoading?: boolean;
  /**
   * A terminal provider/permission boundary. History remains visible inside
   * this same thread, but the composer is deliberately absent so the UI can
   * never imply that a new run would continue the old provider session.
   */
  unavailableReason?: ReactNode;
  quickActionsDisabled?: boolean;
  allowSubmitWhileRunning?: boolean;
  uploadEnabled?: boolean;
  onSend?: AgentThreadSubmit;
  onStop?: () => void;
  queueTasks?: ChatQueuedTask[];
  onSendQueuedTaskNow?: (taskId: string) => Promise<void> | void;
  onEditQueuedTask?: (taskId: string) => Promise<void> | void;
  onRemoveQueuedTask?: (taskId: string) => Promise<void> | void;
  onClearQueuedTasks?: () => Promise<void> | void;
  leftAdornment?: ReactNode;
  draftKey?: string;
  editorKey?: string;
  closeIcon?: "panel" | "back";
  compact?: boolean;
  /** Durable branch emitted by a completed worktree task. */
  checkpointLabel?: string;
}

/**
 * The one interactive Agent conversation surface used by every run entry
 * point. Consumers provide domain adapters (Issue comments, direct task
 * continuations, or Automation runs), while message rendering, tool cards,
 * queue controls, and terminal boundaries stay identical.
 */
export function AgentThreadSurface({
  onClose,
  collapseLabel,
  agentId,
  agentName,
  title,
  description,
  descriptionHint,
  messages,
  pendingTask,
  availability,
  isLoading = false,
  unavailableReason,
  quickActionsDisabled = true,
  allowSubmitWhileRunning = false,
  uploadEnabled = false,
  onSend,
  onStop,
  queueTasks = [],
  onSendQueuedTaskNow,
  onEditQueuedTask,
  onRemoveQueuedTask,
  onClearQueuedTasks,
  leftAdornment,
  draftKey,
  editorKey,
  closeIcon = "panel",
  compact = false,
  checkpointLabel,
}: AgentThreadSurfaceProps) {
  const titleId = useId();
  const canSend = !unavailableReason && !!onSend;
  const canStop = !!pendingTask?.task_id && !!onStop;
  const showComposer = canSend || canStop;
  const hasQueueAction = !!onSendQueuedTaskNow || !!onEditQueuedTask ||
    !!onRemoveQueuedTask || !!onClearQueuedTasks;

  return (
    <aside
      aria-labelledby={titleId}
      className="flex h-full min-h-0 min-w-0 flex-col overflow-hidden bg-page-canvas"
      data-slot="agent-thread-panel"
    >
        <header className={compact ? "shrink-0 border-b px-3 py-2 text-left" : "shrink-0 border-b px-4 py-3 text-left"}>
          <div className="flex min-w-0 items-start gap-3">
            <ActorAvatar
              actorType="agent"
              actorId={agentId}
              size={compact ? "sm" : "md"}
              enableHoverCard
            />
            <div className="min-w-0 flex-1">
              <h2 id={titleId} className="truncate text-body font-medium">{title}</h2>
              {!compact && (description || descriptionHint) && (
                <div className="mt-0.5 min-w-0 text-caption text-muted-foreground">
                  {description && <span className="block">{description}</span>}
                  {descriptionHint && (
                    <span className="mt-0.5 block">{descriptionHint}</span>
                  )}
                </div>
              )}
            </div>
            <Button
              type="button"
              variant="ghost"
              size="icon-xs"
              className="shrink-0 text-muted-foreground"
              aria-label={collapseLabel}
              title={collapseLabel}
              onClick={onClose}
            >
              {closeIcon === "back" ? (
                <ArrowLeft aria-hidden="true" />
              ) : (
                <PanelRightClose aria-hidden="true" />
              )}
            </Button>
          </div>
        </header>

        <div className="flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden @container">
          {isLoading ? (
            <ChatMessageSkeleton />
          ) : (
            <ChatMessageList
              messages={messages}
              pendingTask={pendingTask}
              availability={availability}
              quickActionsDisabled={quickActionsDisabled || !canSend}
              showProcessSteps={!compact}
            />
          )}

          {checkpointLabel ? (
            <Checkpoint
              className="mx-4 mb-2 text-caption"
              data-slot="agent-thread-checkpoint"
              title={checkpointLabel}
            >
              <CheckpointIcon className="size-3.5" />
              <span className="max-w-64 truncate">{checkpointLabel}</span>
            </Checkpoint>
          ) : null}

          {unavailableReason ? (
            <div
              role="alert"
              className="mx-4 mb-4 rounded-lg border border-destructive/30 bg-destructive/5 px-3 py-2 text-caption text-destructive"
            >
              {unavailableReason}
            </div>
          ) : null}
          {showComposer ? (
            <>
              {queueTasks.length > 0 ? (
                <ChatQueue
                  tasks={queueTasks}
                  headStatus={pendingTask?.status}
                  readOnly={!hasQueueAction}
                  sendNowDisabled={!canSend}
                  onSendNow={onSendQueuedTaskNow}
                  onEdit={onEditQueuedTask}
                  onRemove={onRemoveQueuedTask}
                  onClear={onClearQueuedTasks}
                />
              ) : null}
              <ChatInput
                onSend={onSend ?? blockedAgentThreadSubmit}
                uploadEnabled={uploadEnabled && canSend}
                onStop={onStop}
                isRunning={!!pendingTask?.task_id}
                allowSubmitWhileRunning={allowSubmitWhileRunning && canSend}
                disabled={!canSend}
                agentName={agentName}
                leftAdornment={leftAdornment}
                draftKeyOverride={draftKey}
                editorKeyOverride={editorKey ?? draftKey}
              />
            </>
          ) : null}
        </div>
    </aside>
  );
}
