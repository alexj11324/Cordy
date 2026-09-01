"use client";

import type { ComponentProps } from "react";
import { ChatInput } from "./components/chat-input";

/**
 * Agent-page composer.
 *
 * Mapped from the official LobeHub product repo
 * https://github.com/lobehub/lobehub
 * (`https://github.com/lobehub/lobe-chat` 301s to that same repository; it is
 * not a fork). Path:
 * `src/routes/(main)/agent/features/Conversation/MainChatInput`.
 *
 * Thin Agent-route wrapper around the shared Conversation `ChatInput`. Send,
 * drafts, queue, mentions, slash commands, and attachments stay in that
 * shared input. This wrapper is the Agent surface entry so ChatPage does not
 * invent a second composer, and it is the only callsite that opts the body
 * into LobeHub's Lexical editor (`@lobehub/editor`, the same stack as
 * `src/features/ChatInput/InputEditor`).
 */
export function MainChatInput(props: Omit<ComponentProps<typeof ChatInput>, "editorEngine">) {
  return <ChatInput {...props} editorEngine="lexical" />;
}
