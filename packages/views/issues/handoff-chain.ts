import type { IssueActorType, TimelineEntry } from "@patchbay/core/types";

export type HandoffActor = { type: IssueActorType; id: string };
export type HandoffHop = { from: HandoffActor; to: HandoffActor };

function actorFrom(type: unknown, id: unknown): HandoffActor | null {
  if (
    (type !== "member" && type !== "agent" && type !== "team") ||
    typeof id !== "string" ||
    id.length === 0
  ) {
    return null;
  }
  return { type, id };
}

export function issueActor(
  type: IssueActorType | null | undefined,
  id: string | null | undefined,
): HandoffActor | null {
  return actorFrom(type, id);
}

export function reviewHandoffHops(
  timeline: readonly Pick<TimelineEntry, "action" | "details">[],
): HandoffHop[] {
  const hops: HandoffHop[] = [];
  for (const entry of timeline) {
    if (entry.action !== "review_handoff") continue;
    const details = entry.details ?? {};
    const from = actorFrom(details.from_type, details.from_id);
    const to = actorFrom(details.to_type, details.to_id);
    if (from && to) hops.push({ from, to });
  }
  return hops;
}

function actorKey(actor: HandoffActor): string {
  return `${actor.type}:${actor.id}`;
}

export function handoffStackActors(
  hops: readonly HandoffHop[],
  executor: HandoffActor | null,
  reviewer: HandoffActor | null,
): HandoffActor[] {
  const out: HandoffActor[] = [];
  const seen = new Set<string>();
  const push = (actor: HandoffActor | null) => {
    if (!actor || seen.has(actorKey(actor))) return;
    seen.add(actorKey(actor));
    out.push(actor);
  };
  for (const hop of hops) {
    push(hop.from);
    push(hop.to);
  }
  push(executor);
  push(reviewer);
  return out;
}

export function handoffHopsForDisplay(
  hops: readonly HandoffHop[],
  executor: HandoffActor | null,
  reviewer: HandoffActor | null,
): HandoffHop[] {
  if (hops.length > 0) return [...hops];
  if (executor && reviewer && actorKey(executor) !== actorKey(reviewer)) {
    return [{ from: executor, to: reviewer }];
  }
  return [];
}
