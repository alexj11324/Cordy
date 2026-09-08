"use client";

import { type QueryClient } from "@tanstack/react-query";
import { onInboxNew, onInboxSummaryInvalidate } from "../inbox/ws-updaters";
import type {
  InboxItem,
  InboxNewPayload
} from "../types";

import type { ProjectionContext } from "./projection-context";
import { notifyInboxItem } from "./realtime-effects";
export { resolveInboxSourceSlug } from "./realtime-effects";

/**
 * Handles an `inbox:new` event end-to-end: inbox cache invalidation, the
 * focus / mute checks, and the native OS banner. Exported so the handler
 * behavior (not just slug resolution) is testable.
 *
 * Every workspace-scoped read here keys on the ITEM's workspace
 * (`item.workspace_id`), never the currently active one (#3766): the cache
 * invalidation must refresh the source workspace's inbox list / unread
 * count / dock badge, the mute check must honor the source workspace's
 * preference, and the deep link must carry the source workspace's slug.
 */
export async function handleInboxNew(
  qc: QueryClient,
  item: InboxItem,
  isCurrent: () => boolean = () => true,
): Promise<void> {
  const sourceWsId = item.workspace_id;
  if (sourceWsId) onInboxNew(qc, sourceWsId, item);
  // A new item in ANY workspace can light the workspace-switcher dot, so
  // refresh the cross-workspace summary regardless of the active workspace.
  onInboxSummaryInvalidate(qc);
  await notifyInboxItem(qc, item, isCurrent);
}

export function subscribeInboxProjection({ ws, qc, captureGuard }: ProjectionContext) {
  return ws.on("inbox:new", (p) => {
    const { item } = p as InboxNewPayload;
    void handleInboxNew(qc, item, captureGuard());
  });
}
