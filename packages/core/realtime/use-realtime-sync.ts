"use client";

import { useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef } from "react";
import type { StoreApi, UseBoundStore } from "zustand";
import type { WSClient } from "../api/ws-client";
import type { AuthState } from "../auth/store";
import { useHasOnboarded } from "../paths";
import { getCurrentSlug, getCurrentWsId } from "../platform/workspace-storage";
import { invalidateWorkspaceScopedQueries, subscribeCatalogProjection } from "./catalog-projection";
import { subscribeChatProjection } from "./chat-projection";
import { subscribeInboxProjection } from "./inbox-projection";
import { subscribeIssueProjection } from "./issue-projection";
import { createProjectionContext } from "./projection-context";
import { clearDeletedChatSession, subscribeRealtimeEffects } from "./realtime-effects";
import { subscribeWorkspaceProjection } from "./workspace-projection";

export { applyChatCancelFinalizedToCache, applyChatDoneToCache, applyChatMessageToCache, applyChatQuickActionsToCache, applyChatSessionUpdatedToCache, invalidateChatMessageQueries, refetchPendingChatAggregate, removeChatMessageFromCaches } from "./chat-projection";
export { handleInboxNew, resolveInboxSourceSlug } from "./inbox-projection";
export { applyWorkspaceUpdatedToCache } from "./workspace-projection";

export interface RealtimeSyncStores {
  authStore: UseBoundStore<StoreApi<AuthState>>;
}

/** One subscription scope owns the domain projections and recovery for a socket. */
export function useRealtimeSync(
  ws: WSClient | null,
  stores: RealtimeSyncStores,
  onToast?: (message: string, type?: "info" | "error") => void,
) {
  const { authStore } = stores;
  const qc = useQueryClient();
  const hasOnboarded = useHasOnboarded();
  const hasOnboardedRef = useRef(hasOnboarded);
  hasOnboardedRef.current = hasOnboarded;
  const previousWs = useRef<WSClient | null>(null);

  useEffect(() => {
    if (!ws) return;
    const workspaceId = getCurrentWsId();
    const workspaceSlug = getCurrentSlug();
    const scope = createProjectionContext(ws, {
      qc, workspaceId, workspaceSlug, authStore, onToast,
      hasOnboarded: () => hasOnboardedRef.current,
      onChatSessionDeleted: clearDeletedChatSession,
    });
    const subscriptions = [
      subscribeCatalogProjection(scope),
      subscribeIssueProjection(scope),
      subscribeInboxProjection(scope),
      subscribeWorkspaceProjection(scope),
      subscribeRealtimeEffects(scope),
      subscribeChatProjection(scope),
      ws.onReconnect(() => {
        if (scope.isActive()) invalidateWorkspaceScopedQueries(qc, workspaceId);
      }),
    ];
    if (previousWs.current && previousWs.current !== ws && scope.isActive()) {
      invalidateWorkspaceScopedQueries(qc, workspaceId);
    }
    previousWs.current = ws;
    return () => {
      scope.dispose();
      subscriptions.forEach((unsubscribe) => unsubscribe());
    };
  }, [ws, qc, authStore, onToast]);
}
