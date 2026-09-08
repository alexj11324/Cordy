"use client";

import { type QueryClient } from "@tanstack/react-query";
import { issueKeys } from "../issues/queries";
import type {
  MemberAddedPayload,
  WorkspaceUpdatedPayload
} from "../types";
import type { Workspace } from "../types/workspace";
import { workspaceKeys } from "../workspace/queries";

import type { ProjectionContext } from "./projection-context";

/**
 * Apply a workspace:updated event directly to the cached workspace list.
 * If the incoming `issue_prefix` differs from what's currently cached, also
 * invalidates issueKeys.all for that workspace, since every issue's rendered
 * identifier (`MUL-123`) is recomputed from the workspace prefix at read time.
 *
 * If the workspace isn't in the cached list (first observation), we
 * conservatively invalidate — the prefix is effectively "new" relative to
 * what's cached, so any issues already loaded under the old prefix would
 * be stale anyway.
 */
export function applyWorkspaceUpdatedToCache(
  qc: QueryClient,
  payload: WorkspaceUpdatedPayload,
): void {
  const next = payload.workspace;
  if (next?.id) {
    const list = qc.getQueryData<Workspace[]>(workspaceKeys.list());
    const cached = list?.find((w) => w.id === next.id) ?? null;
    if (cached && cached.issue_prefix !== next.issue_prefix) {
      qc.invalidateQueries({ queryKey: issueKeys.all(next.id) });
    }
    if (cached && list) {
      qc.setQueryData<Workspace[]>(
        workspaceKeys.list(),
        list.map((workspace) => (workspace.id === next.id ? next : workspace)),
      );
      return;
    }
    // Do not seed an absent list with one workspace: staleTime is Infinity,
    // so doing so would hide every other membership until a hard refresh.
    qc.invalidateQueries({ queryKey: issueKeys.all(next.id) });
  }
  qc.invalidateQueries({ queryKey: workspaceKeys.list() });
}

export function subscribeWorkspaceProjection({ ws, qc, workspaceId, authStore }: ProjectionContext) {
  const unsubWsUpdated = ws.on("workspace:updated", (p) => {
    applyWorkspaceUpdatedToCache(qc, p as WorkspaceUpdatedPayload);
  });

  const unsubMemberAdded = ws.on("member:added", (p) => {
    const { member } = p as MemberAddedPayload;
    const myUserId = authStore.getState().user?.id;
    if (member.user_id === myUserId) {
      qc.invalidateQueries({ queryKey: workspaceKeys.list() });
      qc.invalidateQueries({ queryKey: workspaceKeys.myInvitations() });
    }
  });

  // invitation:created — notify the invitee of a new pending invitation
  const unsubInvitationCreated = ws.on("invitation:created", () => {
    qc.invalidateQueries({ queryKey: workspaceKeys.myInvitations() });
  });

  // invitation:accepted / declined / revoked — refresh invitation lists
  const unsubInvitationAccepted = ws.on("invitation:accepted", () => {
    const currentWsId = workspaceId;
    if (currentWsId) {
      qc.invalidateQueries({ queryKey: workspaceKeys.invitations(currentWsId) });
      qc.invalidateQueries({ queryKey: workspaceKeys.members(currentWsId) });
    }
  });
  const unsubInvitationDeclined = ws.on("invitation:declined", () => {
    const currentWsId = workspaceId;
    if (currentWsId) {
      qc.invalidateQueries({ queryKey: workspaceKeys.invitations(currentWsId) });
    }
  });
  const unsubInvitationRevoked = ws.on("invitation:revoked", () => {
    qc.invalidateQueries({ queryKey: workspaceKeys.myInvitations() });
  });

  return () => {
    unsubWsUpdated();
    unsubMemberAdded();
    unsubInvitationCreated();
    unsubInvitationAccepted();
    unsubInvitationDeclined();
    unsubInvitationRevoked();
  };
}
