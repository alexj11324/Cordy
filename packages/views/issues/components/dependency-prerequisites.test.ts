import { describe, expect, it } from "vitest";
import type {
  DependencyGraphEdge,
  DependencyGraphNode,
  DependencyGraphResponse,
  Issue,
} from "@patchbay/core/types";
import {
  dependencyPrerequisiteBlockKind,
  selectDependencyPrerequisites,
} from "./dependency-prerequisites";

function issue(id: string, status: string): Issue {
  return {
    id,
    workspace_id: "workspace-1",
    number: Number(id.replace(/\D/g, "")) || 1,
    identifier: id.toUpperCase(),
    title: `${id} title`,
    description: null,
    status,
    priority: "none",
    assignee_type: null,
    assignee_id: null,
    creator_type: "member",
    creator_id: "member-1",
    parent_issue_id: "parent-1",
    project_id: "project-1",
    position: 1,
    stage: null,
    start_date: null,
    due_date: null,
    metadata: {},
    properties: {},
    created_at: "2026-08-29T00:00:00Z",
    updated_at: "2026-08-29T00:00:00Z",
  };
}

function node(tempId: string, status: string, state: DependencyGraphNode["readiness"]["state"], satisfied: number, total: number): DependencyGraphNode {
  const nodeIssue = issue(tempId, status);
  return {
    id: `node-${tempId}`,
    temp_id: tempId,
    issue_id: nodeIssue.id,
    issue: nodeIssue,
    title: nodeIssue.title,
    description: "",
    acceptance_criteria: [],
    context: {},
    outputs: [`${tempId}-output`],
    assignee_type: null,
    assignee_id: null,
    candidate_assignees: [],
    wave: 0,
    status,
    readiness: {
      state,
      gate_open: satisfied === total,
      satisfied_prerequisites: satisfied,
      total_prerequisites: total,
      unlock_condition: `All ${total} hard prerequisites must be Done (${satisfied}/${total} currently satisfied)`,
    },
  };
}

function edge(
  from: string,
  to: string,
  prerequisiteStatus: string,
  satisfied: boolean,
): DependencyGraphEdge {
  return {
    id: `edge-${from}-${to}`,
    plan_id: "plan-1",
    from_issue_id: from,
    to_issue_id: to,
    from,
    to,
    type: "hard",
    reason: `${to} consumes ${from}'s output`,
    consumed_output: `${from}-output`,
    prerequisite_status: prerequisiteStatus,
    satisfied,
    satisfied_prerequisites: satisfied ? 2 : 1,
    total_prerequisites: 2,
    unlock_condition: "All 2 hard prerequisites must be Done",
  };
}

function graph(
  nodes: DependencyGraphNode[],
  edges: DependencyGraphEdge[],
  attentionRequired = false,
): DependencyGraphResponse {
  return {
    plan: {
      id: "plan-1",
      workspace_id: "workspace-1",
      parent_issue_id: "parent-1",
      idempotency_key: "plan-key",
      goal: "goal",
      status: "active",
      attention_required: attentionRequired,
      attention_reason: attentionRequired ? "prerequisite task failed: test failure" : null,
      created_by_type: "member",
      created_by_id: "member-1",
      created_at: "2026-08-29T00:00:00Z",
      updated_at: "2026-08-29T00:00:00Z",
    },
    parent: issue("parent-1", "in_progress"),
    children: nodes.map((item) => item.issue),
    nodes,
    edges,
    waves: [["task-1", "task-2"], ["task-3"]],
    readiness: { total: nodes.length, ready: 0, running: 0, blocked: 1, done: 0, cancelled: 0 },
  };
}

describe("selectDependencyPrerequisites", () => {
  it("keeps all persisted prerequisites when only some are complete", () => {
    const task1 = node("task-1", "done", "done", 0, 0);
    const task2 = node("task-2", "in_progress", "running", 0, 0);
    const task3 = node("task-3", "blocked", "blocked", 1, 2);
    const partial = graph(
      [task1, task2, task3],
      [
        edge(task1.issue_id, task3.issue_id, "done", true),
        edge(task2.issue_id, task3.issue_id, "in_progress", false),
      ],
    );

    const prerequisites = selectDependencyPrerequisites(partial, task3.issue_id);

    expect(prerequisites.map((item) => item.node.issue_id)).toEqual([
      task1.issue_id,
      task2.issue_id,
    ]);
    expect(prerequisites[0]?.edge.satisfied).toBe(true);
    expect(prerequisites[1]?.edge.satisfied).toBe(false);
    expect(task3.readiness).toMatchObject({ state: "blocked", satisfied_prerequisites: 1, total_prerequisites: 2 });
  });

  it("represents the target as unlocked only after every prerequisite is Done", () => {
    const task1 = node("task-1", "done", "done", 0, 0);
    const task2 = node("task-2", "done", "done", 0, 0);
    const task3 = node("task-3", "todo", "ready", 2, 2);
    const complete = graph(
      [task1, task2, task3],
      [
        edge(task1.issue_id, task3.issue_id, "done", true),
        edge(task2.issue_id, task3.issue_id, "done", true),
      ],
    );

    const prerequisites = selectDependencyPrerequisites(complete, task3.issue_id);

    expect(prerequisites).toHaveLength(2);
    expect(prerequisites.every((item) => item.edge.satisfied)).toBe(true);
    expect(task3.readiness).toMatchObject({ state: "ready", gate_open: true, satisfied_prerequisites: 2, total_prerequisites: 2 });
    expect(dependencyPrerequisiteBlockKind(complete, prerequisites[0]!)).toBeNull();
  });

  it("keeps cancellation fail-closed and exposes failure attention from the persisted plan", () => {
    const task1 = node("task-1", "done", "done", 0, 0);
    const task2 = node("task-2", "cancelled", "cancelled", 0, 0);
    const task3 = node("task-3", "blocked", "blocked", 1, 2);
    const cancelled = graph(
      [task1, task2, task3],
      [
        edge(task1.issue_id, task3.issue_id, "done", true),
        edge(task2.issue_id, task3.issue_id, "cancelled", false),
      ],
    );
    const cancelledPrerequisite = selectDependencyPrerequisites(cancelled, task3.issue_id)[1]!;
    expect(dependencyPrerequisiteBlockKind(cancelled, cancelledPrerequisite)).toBe("cancelled");
    expect(task3.readiness.gate_open).toBe(false);

    const failed = graph(
      [task1, node("task-2", "in_progress", "blocked", 0, 0), task3],
      [
        edge(task1.issue_id, task3.issue_id, "done", true),
        edge("task-2", task3.issue_id, "failed", false),
      ],
      true,
    );
    const failedPrerequisite = selectDependencyPrerequisites(failed, task3.issue_id)[1]!;
    expect(dependencyPrerequisiteBlockKind(failed, failedPrerequisite)).toBe("attention");
    expect(task3.readiness.state).toBe("blocked");
  });

  it("returns the explicit no-dependency empty state for roots and refreshed graphs", () => {
    const root = node("task-1", "todo", "ready", 0, 0);
    const rootGraph = graph([root], []);
    expect(selectDependencyPrerequisites(rootGraph, root.issue_id)).toEqual([]);

    const task2 = node("task-2", "done", "done", 0, 0);
    const dependent = node("task-3", "todo", "ready", 1, 1);
    const refreshed = graph(
      [task2, dependent],
      [edge(task2.issue_id, dependent.issue_id, "done", true)],
    );
    expect(selectDependencyPrerequisites(refreshed, dependent.issue_id)).toHaveLength(1);
  });
});
