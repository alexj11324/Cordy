import type { IssueActorType } from "@patchbay/core/types";

export type RoleValue = { type: IssueActorType; id: string } | null;
export type RolePickerKind = "owner" | "reviewer";

export type IssueRoleOptionActor = {
  type: IssueActorType;
  id: string;
  name: string;
  archived?: boolean;
  needsRuntime?: boolean;
};

export type IssueRoleOptionRow =
  | { kind: "unassigned" }
  | { kind: "actor"; actor: IssueRoleOptionActor };

const ACTOR_KIND_ORDER: Record<IssueActorType, number> = {
  member: 0,
  agent: 1,
  team: 2,
};

function sameActor(left: RoleValue, right: RoleValue): boolean {
  return (
    left !== null &&
    right !== null &&
    left.type === right.type &&
    left.id === right.id
  );
}

export function isIssueRoleOptionSelected(
  value: RoleValue,
  row: IssueRoleOptionRow,
): boolean {
  if (row.kind === "unassigned") return value === null;
  return sameActor(value, row.actor);
}

export function buildIssueRoleOptions({
  kind,
  value,
  query,
  actors,
  allowUnassigned = true,
  excludedActor = null,
}: {
  kind: RolePickerKind;
  value: RoleValue;
  query: string;
  actors: readonly IssueRoleOptionActor[];
  allowUnassigned?: boolean;
  excludedActor?: RoleValue;
}): IssueRoleOptionRow[] {
  const needle = query.trim().toLowerCase();
  const visible = actors
    .filter((actor) => !actor.archived)
    .filter((actor) => kind === "reviewer" || actor.type === "member")
    .filter((actor) => !sameActor(actor, excludedActor))
    .filter((actor) => !needle || actor.name.toLowerCase().includes(needle))
    .sort(
      (left, right) =>
        ACTOR_KIND_ORDER[left.type] - ACTOR_KIND_ORDER[right.type] ||
        left.name.localeCompare(right.name),
    );

  const actorRows: IssueRoleOptionRow[] = visible.map((actor) => ({
    kind: "actor",
    actor,
  }));
  if (needle) return actorRows;

  const current = actorRows.find((row) =>
    isIssueRoleOptionSelected(value, row),
  );
  return [
    ...(allowUnassigned ? [{ kind: "unassigned" as const }] : []),
    ...(current ? [current] : []),
    ...actorRows.filter((row) => !isIssueRoleOptionSelected(value, row)),
  ];
}
