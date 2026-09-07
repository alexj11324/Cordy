"use client";

import { useChatStore } from "@patchbay/core/chat";
import { useAgentThreadPanelStore } from "@patchbay/core/agent-thread";
import { useWorkspacePaths } from "@patchbay/core/paths";
import { useNavigation } from "../navigation";
import { ChatFab } from "./components/chat-fab";
import { ChatWindow } from "./components/chat-window";
import { isFloatingChatRouteSuppressed } from "./floating-chat-visibility";

/**
 * Mount point for the floating chat overlay (FAB + window). Rendered once in
 * each app shell's dashboard layout; owns the gates that decide whether the
 * overlay exists at all:
 *
 *  1. The Settings → Chat preference (`floatingChatEnabled`). When a user turns
 *     the floating window off, Chat lives only in its dedicated tab.
 *  2. The Chat tab route itself. On `/:slug/chat` the full-page surface already
 *     owns the conversation, so a floating copy of the same `activeSessionId`
 *     would be pure duplication — hide it there.
 *  3. An open task Agent panel. The corner overlay would cover its composer.
 */
export function FloatingChat() {
  const enabled = useChatStore((s) => s.floatingChatEnabled);
  const agentThreadPath = useAgentThreadPanelStore((s) => s.panel?.routePath);
  const { pathname } = useNavigation();
  const wsPaths = useWorkspacePaths();

  if (!enabled) return null;
  // Conversation surfaces own their available space, including their composer.
  if (isFloatingChatRouteSuppressed(pathname, wsPaths.chat(), agentThreadPath)) return null;

  return (
    <>
      <ChatWindow />
      <ChatFab />
    </>
  );
}
