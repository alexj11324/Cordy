import type { ChatSession } from "@patchbay/core/types";

export const CHAT_ACTIVE_SESSION_STORAGE_PREFIX =
  "patchbay_chat_active_session_v1";

export function chatActiveSessionStorageKey(workspaceId: string): string {
  return `${CHAT_ACTIVE_SESSION_STORAGE_PREFIX}:${workspaceId}`;
}

export type ChatRouteParam = string | string[] | undefined;

export function firstChatRouteParam(value: ChatRouteParam): string | null {
  return typeof value === "string" ? value : value?.[0] ?? null;
}

/**
 * Build the canonical route state for the native chat surface. A persisted
 * session wins over a draft agent selection because a session already has an
 * immutable agent binding; `undefined` clears the stale alternate query key.
 */
export function chatRouteParams(
  activeSessionId: string | null,
  selectedAgentId: string | null,
): { session?: string; agent?: string } {
  if (activeSessionId) {
    return { session: activeSessionId, agent: undefined };
  }
  if (selectedAgentId) {
    return { session: undefined, agent: selectedAgentId };
  }
  return { session: undefined, agent: undefined };
}

export function chatAgentHref(workspaceSlug: string, agentId: string): string {
  return `/${workspaceSlug}/chat?agent=${encodeURIComponent(agentId)}`;
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
