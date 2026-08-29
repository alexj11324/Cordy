import type { IssueAssigneeType, TimelineEntry } from "@patchbay/core/types";

export type HandoffActor = {
  type: IssueAssigneeType;
  id: string;
};

export type HandoffHop = {
  from: HandoffActor;
  to: HandoffActor;
};

function isAssigneeType(value: unknown): value is IssueAssigneeType {
  return value === "member" || value === "agent" || value === "team";
}

function actorFrom(type: unknown, id: unknown): HandoffActor | null {
  if (!isAssigneeType(type) || typeof id !== "string" || id.length === 0) {
    return null;
  }
  return { type, id };
}

export function issueActor(
  type: IssueAssigneeType | null | undefined,
  id: string | null | undefined,
): HandoffActor | null {
  return actorFrom(type, id);
}

/** Chronological review-handoff hops recorded on the issue timeline. */
export function reviewHandoffHops(
  timeline: readonly Pick<TimelineEntry, "action" | "details">[],
): HandoffHop[] {
  const hops: HandoffHop[] = [];
  for (const entry of timeline) {
    if (entry.action !== "review_handoff") continue;
    const details = entry.details ?? {};
    const from = actorFrom(details.from_type, details.from_id);
    const to = actorFrom(details.to_type, details.to_id);
    if (!from || !to) continue;
    hops.push({ from, to });
  }
  return hops;
}

function actorKey(actor: HandoffActor): string {
  return `${actor.type}:${actor.id}`;
}

/**
 * Unique actors for the stacked trigger, in chain order: each hop's
 * from then to, then the current assignee and reviewer if they are new.
 */
export function handoffStackActors(
  hops: readonly HandoffHop[],
  assignee: HandoffActor | null,
  reviewer: HandoffActor | null,
): HandoffActor[] {
  const out: HandoffActor[] = [];
  const seen = new Set<string>();
  const push = (actor: HandoffActor | null) => {
    if (!actor) return;
    const key = actorKey(actor);
    if (seen.has(key)) return;
    seen.add(key);
    out.push(actor);
  };
  for (const hop of hops) {
    push(hop.from);
    push(hop.to);
  }
  push(assignee);
  push(reviewer);
  return out;
}

/**
 * Hops shown in the popover. Timeline hops win; if none exist but both
 * assignee and reviewer are set and different, synthesize one hop so the
 * popover still names the current handoff.
 */
export function handoffHopsForDisplay(
  hops: readonly HandoffHop[],
  assignee: HandoffActor | null,
  reviewer: HandoffActor | null,
): HandoffHop[] {
  if (hops.length > 0) return [...hops];
  if (
    assignee &&
    reviewer &&
    actorKey(assignee) !== actorKey(reviewer)
  ) {
    return [{ from: assignee, to: reviewer }];
  }
  return [];
}
