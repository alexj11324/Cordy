import { describe, expect, it } from "vitest";
import {
  buildIssueRoleOptions,
  type IssueRoleOptionActor,
} from "./issue-role-options";

const actors: IssueRoleOptionActor[] = [
  { type: "member", id: "member-1", name: "Alex" },
  { type: "agent", id: "agent-1", name: "Builder" },
  { type: "agent", id: "agent-old", name: "Old", archived: true },
  { type: "team", id: "team-1", name: "Platform" },
];

describe("issue role picker options", () => {
  it("keeps Unassigned available after a reviewer is selected", () => {
    const rows = buildIssueRoleOptions({
      kind: "reviewer",
      value: { type: "member", id: "member-1" },
      query: "",
      actors,
    });

    expect(rows[0]).toEqual({ kind: "unassigned" });
    expect(rows[1]).toMatchObject({
      kind: "actor",
      actor: { type: "member", id: "member-1" },
    });
  });

  it("offers every supported reviewer kind but excludes archived actors", () => {
    const rows = buildIssueRoleOptions({
      kind: "reviewer",
      value: null,
      query: "",
      actors,
    });

    expect(
      rows.flatMap((row) =>
        row.kind === "actor" ? [`${row.actor.type}:${row.actor.id}`] : [],
      ),
    ).toEqual(["member:member-1", "agent:agent-1", "team:team-1"]);
  });

  it("excludes the executor and Unassigned during a required handoff", () => {
    const rows = buildIssueRoleOptions({
      kind: "reviewer",
      value: null,
      query: "",
      actors,
      allowUnassigned: false,
      excludedActor: { type: "agent", id: "agent-1" },
    });

    expect(rows.some((row) => row.kind === "unassigned")).toBe(false);
    expect(
      rows.some(
        (row) =>
          row.kind === "actor" &&
          row.actor.type === "agent" &&
          row.actor.id === "agent-1",
      ),
    ).toBe(false);
  });

  it("keeps owner selection member-only", () => {
    const rows = buildIssueRoleOptions({
      kind: "owner",
      value: null,
      query: "",
      actors,
    });

    expect(
      rows.flatMap((row) => (row.kind === "actor" ? [row.actor.type] : [])),
    ).toEqual(["member"]);
  });
});
