"use client";

import type { ComponentProps } from "react";
import { ChatInput } from "./components/chat-input";

/**
 * Agent-page composer.
 *
 * Mapped from LobeHub `src/routes/(main)/agent/features/Conversation/MainChatInput`
 * (https://github.com/lobehub/lobehub): a thin Agent-route wrapper around the
 * shared Conversation `ChatInput`. Send, drafts, queue, mentions, slash
 * commands, and attachments stay in that shared input; this wrapper is the
 * Agent surface entry so ChatPage does not invent a second composer.
 */
export function MainChatInput(props: ComponentProps<typeof ChatInput>) {
  return <ChatInput {...props} />;
}
