import type { ChatSession } from "@patchbay/core/types";

export const CHAT_ACTIVE_SESSION_STORAGE_PREFIX =
  "patchbay_chat_active_session_v1";

export function chatActiveSessionStorageKey(workspaceId: string): string {
  return `${CHAT_ACTIVE_SESSION_STORAGE_PREFIX}:${workspaceId}`;
}

/**
 * Restore a session only when it still belongs to the current workspace. A
 * deleted or stale persisted id falls back to the server's newest row, while
 * an empty list keeps the native new-chat state.
 */
export function resolveActiveChatSessionId(
  persistedId: string | null | undefined,
  sessions: readonly Pick<ChatSession, "id">[],
): string | null {
  if (persistedId && sessions.some((session) => session.id === persistedId)) {
    return persistedId;
  }
  return sessions[0]?.id ?? null;
}
