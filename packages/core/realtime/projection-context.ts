import type { QueryClient } from "@tanstack/react-query";
import type { WSClient } from "../api/ws-client";
import { getCurrentSlug, getCurrentWsId } from "../platform/workspace-storage";
import type { RealtimeSyncStores } from "./use-realtime-sync";

interface ProjectionOptions extends RealtimeSyncStores {
  qc: QueryClient;
  workspaceId: string | null;
  workspaceSlug: string | null;
  hasOnboarded: () => boolean;
  onToast?: (message: string, type?: "info" | "error") => void;
  onChatSessionDeleted: (sessionId: string) => void;
}

export interface ProjectionContext extends ProjectionOptions {
  ws: Pick<WSClient, "on" | "onAny">;
  isActive: () => boolean;
  captureGuard: () => () => boolean;
  dispose: () => void;
}

/** Freeze query ownership at subscription time, including work deferred by a handler. */
export function createProjectionContext(ws: WSClient, options: ProjectionOptions): ProjectionContext {
  let disposed = false;
  const isActive = () => !disposed
    && getCurrentWsId() === options.workspaceId
    && getCurrentSlug() === options.workspaceSlug
    && (!ws.subscriptionWorkspaceSlug || ws.subscriptionWorkspaceSlug === options.workspaceSlug);
  const captureGuard = () => {
    const generation = ws.connectionGeneration;
    return () => isActive() && generation === ws.connectionGeneration;
  };
  return {
    ...options,
    isActive,
    captureGuard,
    dispose: () => { disposed = true; },
    ws: {
      on: (event, handler) => ws.on(event, (payload, actorId, actorType) => {
        if (isActive()) return handler(payload, actorId, actorType);
      }),
      onAny: (handler) => ws.onAny((message) => {
        if (isActive()) return handler(message);
      }),
    },
  };
}
