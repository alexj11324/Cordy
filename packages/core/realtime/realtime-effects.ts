"use client";

import { type QueryClient } from "@tanstack/react-query";
import {
  notificationPreferenceKeys,
  notificationPreferenceOptions,
} from "../notification-preferences/queries";
import {
  showWebNotification,
  type SystemNotificationPayload,
} from "../platform/system-notification";
import type {
  InboxItem,
  NotificationPreferenceResponse
} from "../types";
import { workspaceListOptions } from "../workspace/queries";

import { useChatStore } from "../chat";
import { createLogger } from "../logger";
import { resolvePostAuthDestination } from "../paths";
import { defaultStorage } from "../platform/storage";
import { clearWorkspaceStorage } from "../platform/storage-cleanup";
import type {
  InvitationCreatedPayload,
  MemberAddedPayload,
  MemberRemovedPayload,
  WorkspaceDeletedPayload
} from "../types";
import { isWorkspaceDeletePending } from "../workspace/pending-delete";
import { workspaceKeys } from "../workspace/queries";

import type { ProjectionContext } from "./projection-context";
const logger = createLogger("realtime-sync");

export function clearDeletedChatSession(sessionId: string) {
  const state = useChatStore.getState?.();
  if (state?.activeSessionId === sessionId) state.setActiveSession(null);
}

export function subscribeRealtimeEffects({ ws, qc, workspaceId, workspaceSlug, authStore, hasOnboarded, onToast, captureGuard }: ProjectionContext) {
  // --- Side-effect handlers (toast, navigation) ---

  // After the current workspace disappears (deleted or we were kicked out),
  // navigate to another workspace the user still has access to, or to the
  // create-workspace page. We use a full-page navigation: this reliably
  // tears down any in-flight queries / subscriptions tied to the dead
  // workspace without relying on framework-specific routers from here in
  // core.
  const relocateAfterWorkspaceLoss = async (lostWsId: string) => {
    const isCurrent = captureGuard();
    const wsList = await qc.fetchQuery({
      ...workspaceListOptions(),
      staleTime: 0,
    });
    if (!isCurrent()) return;
    const remaining = wsList.filter((w) => w.id !== lostWsId);
    const target = resolvePostAuthDestination(
      remaining,
      hasOnboarded(),
    );
    if (typeof window !== "undefined") {
      window.location.assign(target);
    }
  };

  const unsubWsDeleted = ws.on("workspace:deleted", (p) => {
    const { workspace_id } = p as WorkspaceDeletedPayload;
    // Self-initiated delete: useDeleteWorkspace owns storage cleanup and
    // navigation (both run after the DELETE resolves). Reacting here too
    // would race that flow's navigation with a full-page relocate — the
    // CancelledError + reload combo this guard exists to prevent. This
    // handler only serves deletes initiated elsewhere (other user/device).
    if (isWorkspaceDeletePending(workspace_id)) return;
    // Event payload has UUID; look up slug from cached workspace list
    // since clearWorkspaceStorage keys are namespaced by slug.
    const wsList = qc.getQueryData<{ id: string; slug: string }[]>(workspaceKeys.list()) ?? [];
    const deletedSlug = wsList.find((w) => w.id === workspace_id)?.slug;
    if (deletedSlug) clearWorkspaceStorage(defaultStorage, deletedSlug);
    if (workspaceId === workspace_id) {
      logger.warn("current workspace deleted, switching");
      onToast?.("This workspace was deleted", "info");
      relocateAfterWorkspaceLoss(workspace_id);
    }
  });

  const unsubMemberRemoved = ws.on("member:removed", (p) => {
    const { user_id, workspace_id } = p as MemberRemovedPayload;
    if (workspace_id !== workspaceId) return;
    const myUserId = authStore.getState().user?.id;
    if (user_id === myUserId) {
      const slug = workspaceSlug;
      const wsId = workspaceId;
      if (slug && wsId) {
        clearWorkspaceStorage(defaultStorage, slug);
        logger.warn("removed from workspace, switching");
        onToast?.("You were removed from this workspace", "info");
        relocateAfterWorkspaceLoss(wsId);
      }
    }
  });

  const unsubMemberAdded = ws.on("member:added", (p) => {
    const { member, workspace_name } = p as MemberAddedPayload;
    const myUserId = authStore.getState().user?.id;
    if (member.user_id === myUserId) {
      onToast?.(
        `You joined ${workspace_name ?? "a workspace"}`,
        "info",
      );
    }
  });

  // invitation:created — notify the invitee of a new pending invitation
  const unsubInvitationCreated = ws.on("invitation:created", (p) => {
    const { workspace_name } = p as InvitationCreatedPayload;
    onToast?.(
      `You were invited to ${workspace_name ?? "a workspace"}`,
      "info",
    );
  });

  return () => { unsubWsDeleted(); unsubMemberRemoved(); unsubMemberAdded(); unsubInvitationCreated(); };
}

/**
 * Resolves the slug of the workspace an inbox item originated from, via the
 * cached workspace list (fetched once when the cache is cold).
 *
 * Desktop notification routing must pin to the *source* workspace of the
 * inbox item, not the currently active one: the user can be on workspace B
 * when an `inbox:new` for workspace A arrives, and macOS Notification Center
 * holds banners across workspace switches. Returns null when the workspace
 * cannot be resolved — callers must NOT fall back to the current slug (that
 * recreates the wrong-workspace routing this exists to prevent, #3766) and
 * should show the notification without a deep link instead.
 */
export async function resolveInboxSourceSlug(
  qc: QueryClient,
  workspaceId: string,
): Promise<string | null> {
  if (!workspaceId) return null;
  try {
    const workspaces = await qc.ensureQueryData(workspaceListOptions());
    return workspaces?.find((w) => w.id === workspaceId)?.slug ?? null;
  } catch {
    // Workspace list unavailable (e.g. network hiccup): degrade to a
    // link-less notification rather than guessing a slug.
    return null;
  }
}

export async function notifyInboxItem(
  qc: QueryClient,
  item: InboxItem,
  isCurrent: () => boolean = () => true,
): Promise<void> {
  const sourceWsId = item.workspace_id;
  // Fire a native OS notification only when the app isn't focused. When
  // the user is already looking at Orvilo, the inbox sidebar's unread
  // styling is enough — no need to interrupt with a banner. `desktopAPI`
  // is injected by the preload script; its absence (web app) skips silently.
  if (typeof document !== "undefined" && document.hasFocus()) return;
  // Resolve the source workspace's slug once: it pins BOTH the mute check
  // and the deep link to the workspace the inbox item BELONGS to, never the
  // currently active one. Reading `getCurrentSlug()` here was the source of
  // wrong-workspace routing (#3766): an `inbox:new` from workspace A arriving
  // while workspace B is active emitted a notification carrying B's slug and
  // A's issue id, deep-linking to an issue B doesn't have.
  const slug = await resolveInboxSourceSlug(qc, sourceWsId);
  // Respect the SOURCE workspace's system-notification preference. Keying the
  // query on `sourceWsId` is not enough: the request resolves its workspace
  // from the `X-Workspace-Slug` header, which follows the ACTIVE workspace —
  // so a cold-cache lookup while viewing B would read B's mute setting and
  // cache it under A's key. Passing the source slug scopes the fetch to A.
  // When the slug can't be resolved we read only an already-warm cache
  // (populated earlier with the correct workspace context) rather than fetch
  // with the wrong one; on network failure we fall through to the default
  // ("all") rather than swallow the banner.
  if (sourceWsId) {
    try {
      const prefData = slug
        ? await qc.ensureQueryData(
          notificationPreferenceOptions(sourceWsId, slug),
        )
        : qc.getQueryData<NotificationPreferenceResponse>(
          notificationPreferenceKeys.all(sourceWsId),
        );
      if (prefData?.preferences?.system_notifications === "muted") return;
    } catch {
      // Fall through with default behavior.
    }
  }
  // `issueKey` matches the inbox page's URL selector (issue id when the
  // item is attached to an issue, otherwise the inbox item id). `itemId`
  // is the inbox row's own id, needed to fire markInboxRead on click.
  // A null slug (workspace list unavailable / item from a workspace this
  // client can't see) still shows the banner — the user should learn about
  // the inbox item — but with an empty slug so the click is a no-op
  // (the inbox bridge ignores empty slugs) instead of routing wrong.
  if (!isCurrent()) return;
  const payload: SystemNotificationPayload = {
    slug: slug ?? "",
    itemId: item.id,
    issueKey: item.issue_id ?? item.id,
    title: item.title,
    body: item.body ?? "",
  };
  const desktopAPI = (
    globalThis as unknown as {
      desktopAPI?: {
        showNotification?: (payload: SystemNotificationPayload) => void;
      };
    }
  ).desktopAPI;
  if (desktopAPI?.showNotification) {
    // Desktop: native OS banner rendered by the Electron main process.
    desktopAPI.showNotification(payload);
    return;
  }
  // Web: the browser Notification API. No-op without granted permission or on
  // SSR — the in-app inbox + unread badge still reflect the new item.
  showWebNotification(payload);
}
