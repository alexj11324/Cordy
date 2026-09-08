"use client";

import { type QueryClient } from "@tanstack/react-query";
import { agentThreadKeys } from "../agent-thread/queries";
import {
  agentActivityKeys,
  agentRunCountsKeys,
  agentTasksKeys,
  agentTaskSnapshotKeys,
  workspaceWorkingAgentsKeys,
} from "../agents/queries";
import { automationKeys } from "../automations/queries";
import {
  chatKeys
} from "../chat/queries";
import { dingtalkKeys } from "../dingtalk/queries";
import { githubKeys } from "../github/queries";
import { inboxKeys } from "../inbox/queries";
import { onInboxInvalidate, onInboxSummaryInvalidate } from "../inbox/ws-updaters";
import { issueStatusKeys } from "../issue-statuses/queries";
import { issueKeys } from "../issues/queries";
import { labelKeys } from "../labels/queries";
import { larkKeys } from "../lark/queries";
import { pinKeys } from "../pins/queries";
import { projectKeys } from "../projects/queries";
import { propertyKeys } from "../properties/queries";
import { runtimeKeys } from "../runtimes/queries";
import { slackKeys } from "../slack/queries";
import { telegramKeys } from "../telegram/queries";
import { wecomKeys } from "../wecom/queries";
import { weixinKeys } from "../weixin/queries";
import { workProductKeys } from "../work-products/queries";
import { workspaceKeys } from "../workspace/queries";

import type { ProjectionContext } from "./projection-context";
export function invalidateWorkspaceScopedQueries(qc: QueryClient, wsId: string | null): void {
  if (wsId) {
    qc.invalidateQueries({ queryKey: issueKeys.all(wsId) });
    qc.invalidateQueries({ queryKey: inboxKeys.all(wsId) });
    qc.invalidateQueries({ queryKey: workspaceKeys.agents(wsId) });
    qc.invalidateQueries({ queryKey: workspaceKeys.members(wsId) });
    qc.invalidateQueries({ queryKey: workspaceKeys.teams(wsId) });
    qc.invalidateQueries({ queryKey: workspaceKeys.skills(wsId) });
    qc.invalidateQueries({ queryKey: workspaceKeys.invitations(wsId) });
    qc.invalidateQueries({ queryKey: projectKeys.all(wsId) });
    qc.invalidateQueries({ queryKey: runtimeKeys.all(wsId) });
    qc.invalidateQueries({ queryKey: automationKeys.all(wsId) });
    qc.invalidateQueries({ queryKey: agentTaskSnapshotKeys.all(wsId) });
    qc.invalidateQueries({ queryKey: workspaceWorkingAgentsKeys.all(wsId) });
    qc.invalidateQueries({ queryKey: agentActivityKeys.all(wsId) });
    qc.invalidateQueries({ queryKey: agentRunCountsKeys.all(wsId) });
    qc.invalidateQueries({ queryKey: chatKeys.all(wsId) });
    qc.invalidateQueries({ queryKey: agentThreadKeys.all(wsId) });
    qc.invalidateQueries({ queryKey: labelKeys.all(wsId) });
    qc.invalidateQueries({ queryKey: propertyKeys.all(wsId) });
    qc.invalidateQueries({ queryKey: weixinKeys.all(wsId) });
    // A catalog edit missed while disconnected would otherwise sit behind the
    // 5-minute staleTime — long enough to offer a status the server already
    // archived, or to keep painting its old name.
    qc.invalidateQueries({ queryKey: issueStatusKeys.all(wsId) });
    qc.invalidateQueries({ queryKey: workProductKeys.all(wsId) });
  }
  // Cross-workspace, so outside the wsId guard: a reconnect may have missed
  // inbox events from any workspace, so re-pull the switcher-dot summary.
  onInboxSummaryInvalidate(qc);
  // Per-issue caches are keyed without wsId, so the issueKeys.all(wsId)
  // prefix above does not reach them. They rely entirely on WS events for
  // freshness (staleTime: Infinity), so events missed while disconnected
  // left them stale until a full reload — the inbox showed an agent's new
  // comment while the issue timeline didn't (#3953). Inactive caches only
  // get marked stale here and refetch on next mount; the one mounted issue
  // refetches immediately, same as its own useWSReconnect already does.
  qc.invalidateQueries({ queryKey: issueKeys.timelineAll() });
  qc.invalidateQueries({ queryKey: issueKeys.reactionsAll() });
  qc.invalidateQueries({ queryKey: issueKeys.subscribersAll() });
  qc.invalidateQueries({ queryKey: issueKeys.usageAll() });
  qc.invalidateQueries({ queryKey: issueKeys.attachmentsAll() });
  qc.invalidateQueries({ queryKey: issueKeys.tasksAll() });
  // Per-chat-session caches are also keyed without wsId, so the
  // chatKeys.all(wsId) prefix above only reaches session lists / aggregates.
  // Message streams rely on WS invalidation with staleTime: Infinity; recover
  // sessions that missed chat/task events while the socket was disconnected.
  qc.invalidateQueries({ queryKey: chatKeys.messagesAll() });
  qc.invalidateQueries({ queryKey: chatKeys.messagesPageAll() });
  qc.invalidateQueries({ queryKey: chatKeys.pendingTaskAll() });
  qc.invalidateQueries({ queryKey: chatKeys.taskMessagesAll() });
  // A chat:cancel_finalized broadcast missed while disconnected is exactly
  // what the durable draft-restore rows exist for (#5219) — re-pull them so
  // a mounted composer recovers the prompt without a remount.
  qc.invalidateQueries({ queryKey: chatKeys.draftRestoresAll() });
  qc.invalidateQueries({ queryKey: workspaceKeys.list() });
}

function invalidateTeamMemberStatusQueries(qc: QueryClient, wsId: string): void {
  qc.invalidateQueries({
    predicate: (query) => {
      const key = query.queryKey;
      return (
        key[0] === "workspaces" &&
        key[1] === wsId &&
        key[2] === "teams" &&
        key[4] === "members-status"
      );
    },
  });
}

export function subscribeCatalogProjection({ ws, qc, workspaceId, authStore, captureGuard }: ProjectionContext) {
  const refreshMap: Record<string, () => void> = {
    inbox: () => {
      const wsId = workspaceId;
      if (wsId) onInboxInvalidate(qc, wsId);
      // inbox:read / inbox:archived / inbox:unarchived / batch events arrive
      // here. They can originate from a workspace other than the active one
      // (personal events fan out to all the user's connections), so always
      // refresh the cross-workspace summary — its dot must clear when another
      // workspace's items are read/archived, and light again when an unread
      // item is restored from the archive.
      onInboxSummaryInvalidate(qc);
    },
    agent: () => {
      const wsId = workspaceId;
      if (wsId) {
        qc.invalidateQueries({ queryKey: workspaceKeys.agents(wsId) });
        qc.invalidateQueries({ queryKey: workspaceWorkingAgentsKeys.all(wsId) });
        // Team members status is derived per agent, so any agent
        // change (status flip, archive, runtime swap) needs to refresh the
        // per-team members-status cache without refetching the static team
        // list summary.
        invalidateTeamMemberStatusQueries(qc, wsId);
      }
    },
    member: () => {
      const wsId = workspaceId;
      if (wsId) qc.invalidateQueries({ queryKey: workspaceKeys.members(wsId) });
    },
    // workspace:updated is handled by the specific handler below
    // (compares prefixes to decide whether to also invalidate issues).
    // This generic fallback still fires for workspace:deleted (paired
    // with the specific navigation handler) and any future workspace:*
    // events without dedicated handlers.
    workspace: () => {
      qc.invalidateQueries({ queryKey: workspaceKeys.list() });
    },
    skill: () => {
      const wsId = workspaceId;
      if (wsId) qc.invalidateQueries({ queryKey: workspaceKeys.skills(wsId) });
    },
    project: () => {
      const wsId = workspaceId;
      if (wsId) qc.invalidateQueries({ queryKey: projectKeys.all(wsId) });
    },
    team: () => {
      const wsId = workspaceId;
      if (wsId) {
        qc.invalidateQueries({ queryKey: workspaceKeys.teams(wsId) });
        // team:deleted triggers assignee transfer — refresh issues too.
        qc.invalidateQueries({ queryKey: issueKeys.all(wsId) });
      }
    },
    label: () => {
      // Label catalogs are independently scoped to issues, agents, and
      // skills. The generic event prefix does not carry the scope into this
      // dispatcher, so refresh all three resource projections. Issue rows
      // embed label snapshots; agent/skill label pickers use their resource
      // cache plus the shared label query tree.
      const wsId = workspaceId;
      if (wsId) {
        qc.invalidateQueries({ queryKey: ["labels", wsId] });
        qc.invalidateQueries({ queryKey: issueKeys.all(wsId) });
        qc.invalidateQueries({ queryKey: workspaceKeys.agents(wsId) });
        qc.invalidateQueries({ queryKey: workspaceKeys.skills(wsId) });
      }
    },
    // The issue status catalog (MUL-6243). An admin edits it in the settings
    // page; every other tab and device is rendering statuses out of it.
    //
    // Invalidate only — the event carries no entry to merge. Issue caches are
    // deliberately NOT dragged along: a row stores the status KEY, and its
    // name, color and category are resolved from this catalog at render time
    // (`useStatusLabel`, `colorOf`), so refetching the catalog is what makes a
    // rename repaint. Pulling every board and list with it would turn one
    // admin rename into a workspace-wide refetch storm on every connected
    // client. (MUL-6458)
    issue_status: () => {
      const wsId = workspaceId;
      if (wsId) qc.invalidateQueries({ queryKey: issueStatusKeys.all(wsId) });
    },
    pin: () => {
      const wsId = workspaceId;
      const userId = authStore.getState().user?.id;
      if (wsId && userId) qc.invalidateQueries({ queryKey: pinKeys.all(wsId, userId) });
    },
    daemon: () => {
      const wsId = workspaceId;
      if (wsId) {
        qc.invalidateQueries({ queryKey: runtimeKeys.all(wsId) });
        // Runtime online/offline transitions move the derived status
        // for every agent that hosts on this runtime, which shifts the
        // working/idle/offline pill on the team page.
        invalidateTeamMemberStatusQueries(qc, wsId);
      }
    },
    automation: () => {
      const wsId = workspaceId;
      if (wsId) qc.invalidateQueries({ queryKey: automationKeys.all(wsId) });
    },
    github_installation: () => {
      const wsId = workspaceId;
      // Installation changes also change the repository choices available
      // to automation triggers, so invalidate the whole provider namespace.
      if (wsId) qc.invalidateQueries({ queryKey: githubKeys.all(wsId) });
    },
    lark_installation: () => {
      const wsId = workspaceId;
      if (wsId) qc.invalidateQueries({ queryKey: larkKeys.installations(wsId) });
    },
    slack_installation: () => {
      const wsId = workspaceId;
      // A newly connected bot contributes a new public-channel catalog.
      if (wsId) qc.invalidateQueries({ queryKey: slackKeys.all(wsId) });
    },
    dingtalk_installation: () => {
      const wsId = workspaceId;
      if (wsId) qc.invalidateQueries({ queryKey: dingtalkKeys.installations(wsId) });
    },
    dingtalk_group_route: () => {
      const wsId = workspaceId;
      if (wsId) qc.invalidateQueries({ queryKey: dingtalkKeys.groupRoutes(wsId) });
    },
    vcs_connection: () => {
      const wsId = workspaceId;
      if (wsId) qc.invalidateQueries({ queryKey: ["vcs", wsId] });
    },
    wecom_installation: () => {
      const wsId = workspaceId;
      if (wsId) qc.invalidateQueries({ queryKey: wecomKeys.installations(wsId) });
    },
    weixin_installation: () => {
      const wsId = workspaceId;
      if (wsId) qc.invalidateQueries({ queryKey: weixinKeys.installations(wsId) });
    },
    telegram_installation: () => {
      const wsId = workspaceId;
      if (wsId) qc.invalidateQueries({ queryKey: telegramKeys.installations(wsId) });
    },
    pull_request: () => {
      // A PR is a Work Product now, and its card is embedded in the issue's
      // product list. That list is keyed by issue id, not workspace, so
      // every issue's list is invalidated — the open issue detail page is
      // the only one that refetches.
      qc.invalidateQueries({ queryKey: ["work-products", "issue"] });
      const wsId = workspaceId;
      if (wsId) qc.invalidateQueries({ queryKey: workProductKeys.all(wsId) });
    },
    // Powers the agent presence cache: any task lifecycle change
    // (dispatch / completed / failed / cancelled) refreshes the
    // workspace-wide agent-task-snapshot query so per-agent presence
    // reflects the change. task:message is NOT in this prefix path — it
    // stays in specificEvents to avoid an invalidate storm during long runs.
    task: () => {
      const wsId = workspaceId;
      if (!wsId) return;
      qc.invalidateQueries({ queryKey: agentTaskSnapshotKeys.list(wsId) });
      qc.invalidateQueries({ queryKey: workspaceWorkingAgentsKeys.all(wsId) });
      // The Table working-agent shortcut derives an assignee set from the
      // projection above. Refresh its server-owned graph alongside that set
      // so rows/groups/facets cannot remain on an old task transition while
      // the projection refetches (global staleTime is Infinity).
      qc.invalidateQueries({ queryKey: issueKeys.tableAll(wsId) });
      // 30d activity series shares the same lifecycle signal — any task
      // completion / failure shifts the histogram. (Dispatch alone
      // doesn't change a completed_at-anchored series, but invalidating
      // here keeps the WS-handler shape uniform; the resulting refetch
      // is cheap.) Both the list (trailing 7d slice) and the detail
      // panel read off this single cache.
      qc.invalidateQueries({ queryKey: agentActivityKeys.last30d(wsId) });
      // 30-day run count likewise increments per task lifecycle event.
      qc.invalidateQueries({ queryKey: agentRunCountsKeys.last30d(wsId) });
      // Per-agent task list (Activity tab "Recent work"). Prefix match
      // catches every agent's list — the per-agent detail key sits
      // under agentTasks/<wsId>/<agentId>.
      qc.invalidateQueries({ queryKey: agentTasksKeys.all(wsId) });
      // Per-issue task list (issue-detail Execution log). Prefix match
      // across all issues — keeps the contract "any task: event makes
      // every list-of-tasks query stale" so cache stays fresh even
      // when the relevant component isn't currently mounted.
      qc.invalidateQueries({ queryKey: ["issues", "tasks"] });
      // Agent Thread responses aggregate the entire continuation chain and
      // derive their pending/head state from task lifecycle fields. Keep an
      // open issue conversation in step with queued/running/terminal events.
      qc.invalidateQueries({ queryKey: agentThreadKeys.all(wsId) });
      // Per-issue token usage card (issue-detail right rail). Same
      // shape as the tasks invalidation above — any task lifecycle
      // event shifts the aggregated usage numbers.
      qc.invalidateQueries({ queryKey: ["issues", "usage"] });
      // Team members-status reads the same task lifecycle to flip
      // working ↔ idle for each agent member.
      invalidateTeamMemberStatusQueries(qc, wsId);
      // Comment trigger previews answer "who would a send wake right
      // now" — the pending-task dedup guard makes that answer
      // queue-dependent, so any task lifecycle change must refresh an
      // open composer's chips (e.g. an agent finishing its run becomes
      // triggerable again mid-typing).
      qc.invalidateQueries({ queryKey: issueKeys.commentTriggerPreviewAll() });
      // Issue-trigger previews (assign/status/create/batch) are deliberately
      // NOT invalidated here. Unlike comment triggers, the assign source
      // (create / assignee change) cancels existing tasks before enqueuing, so
      // a task event can never change its verdict; only the status source's
      // pending dedup could, and that preview is advisory — the write path
      // re-evaluates authoritatively, so a rare stale label is harmless.
      // Refetching every mounted preview on every workspace task event caused
      // visible flicker, so the preview now refetches only on input change
      // (signature), mirroring its query design (MUL-3375).
    },
  };

  const timers = new Map<string, { timer: ReturnType<typeof setTimeout>; isCurrent: () => boolean }>();
  const coalescedRefresh = (prefix: string, fn: () => void) => {
    // Keep the first deadline so a continuous event stream still refreshes.
    const existing = timers.get(prefix);
    if (existing?.isCurrent()) return;
    if (existing) clearTimeout(existing.timer);
    const isCurrent = captureGuard();
    timers.set(prefix, {
      isCurrent,
      timer: setTimeout(() => {
        timers.delete(prefix);
        if (isCurrent()) fn();
      }, 100),
    });
  };

  // Event types handled by specific handlers below -- skip generic refresh
  const specificEvents = new Set([
    "workspace:updated",
    "issue:updated", "issue:created", "issue:deleted", "issue_attachments:changed", "issue_labels:changed", "issue_metadata:changed", "issue_properties:changed", "property:created", "property:updated", "inbox:new",
    "comment:created", "comment:updated", "comment:deleted",
    "comment:resolved", "comment:unresolved",
    "activity:created",
    "reaction:added", "reaction:removed",
    "issue_reaction:added", "issue_reaction:removed",
    "subscriber:added", "subscriber:removed",
    "daemon:heartbeat",
    // Chat events are handled explicitly below; do not double-invalidate.
    "chat:message", "chat:done", "chat:quick_actions", "chat:cancel_finalized", "chat:session_read",
    "chat:session_created", "chat:session_deleted", "chat:session_updated",
    // task:message stays out of the prefix path because it fires per
    // streamed message during a long run — invalidating the snapshot on
    // every message would flood the network. Specific chat handlers below
    // still receive it via ws.on() (a separate subscription channel).
    "task:message",
    // task:completed / task:failed deliberately NOT here. They go through
    // both the task-prefix invalidate (refreshes the agent-task-snapshot
    // cache) AND the chat-specific ws.on() handlers below. The two
    // channels are independent — onAny dispatch and ws.on are separate
    // subscriptions.
  ]);

  const unsubAny = ws.onAny((msg) => {
    if (specificEvents.has(msg.type)) return;
    const prefix = msg.type.split(":")[0] ?? "";
    const refresh = refreshMap[prefix];
    if (refresh) coalescedRefresh(prefix, refresh);
  });

  return () => {
    unsubAny();
    timers.forEach(({ timer }) => clearTimeout(timer));
    timers.clear();
  };
}
