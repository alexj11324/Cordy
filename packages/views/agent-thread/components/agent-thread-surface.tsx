"use client";

import type { ComponentProps, ReactNode } from "react";
import type { AgentAvailability } from "@patchbay/core/agents";
import type {
  ChatMessage,
  ChatPendingTask,
  ChatQueuedTask,
} from "@patchbay/core/types";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@patchbay/ui/components/ui/dialog";
import { ActorAvatar } from "../../common/actor-avatar";
import { ChatInput } from "../../chat/components/chat-input";
import { ChatQueue } from "../../chat/components/chat-queue";
import {
  ChatMessageList,
  ChatMessageSkeleton,
} from "../../chat/components/chat-message-list";

export type AgentThreadSubmit = ComponentProps<typeof ChatInput>["onSend"];

export interface AgentThreadSurfaceProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  agentId: string;
  agentName: string;
  userId?: string;
  userName?: string;
  title: ReactNode;
  description?: ReactNode;
  descriptionHint?: ReactNode;
  messages: ChatMessage[];
  messageActors?: ComponentProps<typeof ChatMessageList>["messageActors"];
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
  chooseFollowUp?: boolean;
  onSend?: AgentThreadSubmit;
  onSteer?: AgentThreadSubmit;
  onStop?: () => void;
  queueTasks?: ChatQueuedTask[];
  onEditQueuedTask?: (taskId: string) => Promise<void> | void;
  onRemoveQueuedTask?: (taskId: string) => Promise<void> | void;
  onClearQueuedTasks?: () => Promise<void> | void;
  leftAdornment?: ReactNode;
  draftKey?: string;
  editorKey?: string;
}

/**
 * The one interactive Agent conversation surface used by every run entry
 * point. Consumers provide domain adapters (Issue comments, direct task
 * continuations, or Automation runs), while message rendering, tool cards,
 * queue controls, and terminal boundaries stay identical.
 */
export function AgentThreadSurface({
  open,
  onOpenChange,
  agentId,
  agentName,
  userId,
  userName,
  title,
  description,
  descriptionHint,
  messages,
  messageActors,
  pendingTask,
  availability,
  isLoading = false,
  unavailableReason,
  quickActionsDisabled = true,
  allowSubmitWhileRunning = false,
  chooseFollowUp = false,
  onSend,
  onSteer,
  onStop,
  queueTasks = [],
  onEditQueuedTask,
  onRemoveQueuedTask,
  onClearQueuedTasks,
  leftAdornment,
  draftKey,
  editorKey,
}: AgentThreadSurfaceProps) {
  const hasComposer = !unavailableReason && !!onSend;
  const queueHandlersReady =
    !!onEditQueuedTask && !!onRemoveQueuedTask && !!onClearQueuedTasks;

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="flex h-[min(52rem,90svh)] w-[min(52rem,calc(100vw-2rem))] max-w-none flex-col gap-0 overflow-hidden p-0">
        <DialogHeader className="shrink-0 border-b px-5 py-3.5 text-left">
          <div className="flex items-start gap-3 pr-8">
            <ActorAvatar
              actorType="agent"
              actorId={agentId}
              size="md"
              enableHoverCard
            />
            <div className="min-w-0 flex-1">
              <DialogTitle className="truncate text-body">{title}</DialogTitle>
              {(description || descriptionHint) && (
                <DialogDescription className="mt-0.5 text-caption">
                  {description && <span className="block">{description}</span>}
                  {descriptionHint && (
                    <span className="mt-0.5 block">{descriptionHint}</span>
                  )}
                </DialogDescription>
              )}
            </div>
          </div>
        </DialogHeader>

        <div className="flex min-h-0 flex-1 flex-col @container">
          {isLoading ? (
            <ChatMessageSkeleton />
          ) : (
            <ChatMessageList
              messages={messages}
              messageActors={messageActors}
              agentId={agentId}
              agentName={agentName}
              userId={userId}
              userName={userName}
              pendingTask={pendingTask}
              availability={availability}
              quickActionsDisabled={quickActionsDisabled || !hasComposer}
            />
          )}

          {unavailableReason ? (
            <div
              role="alert"
              className="mx-4 mb-4 rounded-lg border border-destructive/30 bg-destructive/5 px-3 py-2 text-caption text-destructive"
            >
              {unavailableReason}
            </div>
          ) : hasComposer ? (
            <ChatInput
              onSend={onSend}
              onSteer={onSteer}
              onStop={onStop}
              isRunning={!!pendingTask?.task_id}
              allowSubmitWhileRunning={allowSubmitWhileRunning}
              chooseFollowUp={chooseFollowUp}
              queueSlot={
                queueTasks.length > 0 ? (
                  <ChatQueue
                    tasks={queueTasks}
                    headStatus={pendingTask?.status}
                    readOnly={!queueHandlersReady}
                    onEdit={queueHandlersReady ? onEditQueuedTask : undefined}
                    onRemove={
                      queueHandlersReady ? onRemoveQueuedTask : undefined
                    }
                    onClear={
                      queueHandlersReady ? onClearQueuedTasks : undefined
                    }
                  />
                ) : null
              }
              agentName={agentName}
              leftAdornment={
                leftAdornment ?? (
                  <ActorAvatar
                    actorType="agent"
                    actorId={agentId}
                    size="lg"
                    profileLink={false}
                    enableHoverCard
                  />
                )
              }
              draftKeyOverride={draftKey}
              editorKeyOverride={editorKey ?? draftKey}
            />
          ) : null}
        </div>
      </DialogContent>
    </Dialog>
  );
}
