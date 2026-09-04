import { describe, expect, it } from "vitest";
import type { Issue } from "@patchbay/core/types";
import {
  filterIssuesByScope,
  issueActorForRole,
  issueMatchesScope,
} from "./issue-scope";

type RoleIssue = Pick<
  Issue,
  "owner_type" | "owner_id" | "executor_type" | "executor_id"
>;

function issue(overrides: Partial<RoleIssue> = {}): RoleIssue {
  return {
    owner_type: null,
    owner_id: null,
    executor_type: null,
    executor_id: null,
    ...overrides,
  };
}

describe("workspace issue role scopes", () => {
  it("matches Members by owner_type only", () => {
    const memberOwnedAndExecuted = issue({
      owner_type: "member",
      owner_id: "member-1",
      executor_type: "agent",
      executor_id: "agent-1",
    });
    const agentExecutedOnly = issue({
      executor_type: "agent",
      executor_id: "agent-2",
    });

    expect(issueMatchesScope(memberOwnedAndExecuted, "members")).toBe(true);
    expect(issueMatchesScope(agentExecutedOnly, "members")).toBe(false);
    expect(
      filterIssuesByScope(
        [memberOwnedAndExecuted, agentExecutedOnly],
        "members",
      ),
    ).toEqual([memberOwnedAndExecuted]);
  });

  it("matches Agents by executor_type for both agents and teams", () => {
    const agent = issue({ executor_type: "agent", executor_id: "agent-1" });
    const team = issue({ executor_type: "team", executor_id: "team-1" });
    const memberOwnedOnly = issue({
      owner_type: "member",
      owner_id: "member-1",
    });

    expect(issueMatchesScope(agent, "agents")).toBe(true);
    expect(issueMatchesScope(team, "agents")).toBe(true);
    expect(issueMatchesScope(memberOwnedOnly, "agents")).toBe(false);
    expect(
      filterIssuesByScope([agent, team, memberOwnedOnly], "agents"),
    ).toEqual([agent, team]);
  });

  it("does not substitute one role when the requested role is absent", () => {
    const memberOwnedOnly = issue({
      owner_type: "member",
      owner_id: "member-1",
    });
    const agentExecutedOnly = issue({
      executor_type: "agent",
      executor_id: "agent-1",
    });

    expect(issueActorForRole(memberOwnedOnly, "executor")).toBeNull();
    expect(issueActorForRole(agentExecutedOnly, "owner")).toBeNull();
    expect(issueActorForRole(memberOwnedOnly, "owner")).toEqual({
      type: "member",
      id: "member-1",
    });
    expect(issueActorForRole(agentExecutedOnly, "executor")).toEqual({
      type: "agent",
      id: "agent-1",
    });
  });
});
