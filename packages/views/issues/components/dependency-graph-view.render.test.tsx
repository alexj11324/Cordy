import { fireEvent, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type {
  DependencyGraphEdge,
  DependencyGraphNode,
  DependencyGraphResponse,
} from "@patchbay/core/types";
import { renderWithI18n } from "../../test/i18n";
import { DependencyGraphView } from "./dependency-graph-view";

const queryState = vi.hoisted(() => ({
  current: {
    data: [] as unknown[],
    isLoading: false,
    isError: false,
    refetch: vi.fn(),
  },
}));

vi.mock("@patchbay/core/dependency-graphs", () => ({
  dependencyGraphKeys: { all: (workspaceId: string) => ["dependency-graphs", workspaceId] },
  dependencyGraphsOptions: () => ({ queryKey: ["dependency-graphs"] }),
}));
vi.mock("@patchbay/core/hooks", () => ({ useWorkspaceId: () => "workspace-1" }));
vi.mock("@patchbay/core/realtime", () => ({
  useWSEvent: () => {},
  useWSReconnect: () => {},
}));
vi.mock("@patchbay/core/paths", () => ({
  useWorkspacePaths: () => ({
    issueDetail: (id: string) => `/acme/issues/${id}`,
  }),
}));
vi.mock("@tanstack/react-query", () => ({
  useQuery: () => queryState.current,
  useQueryClient: () => ({ invalidateQueries: vi.fn() }),
}));
vi.mock("@patchbay/ui/components/ui/button", () => ({
  Button: ({ children, ...props }: React.ButtonHTMLAttributes<HTMLButtonElement>) => (
    <button {...props}>{children}</button>
  ),
}));
vi.mock("../../navigation", () => ({
  AppLink: ({ children, newTabTitle: _newTabTitle, ...props }: React.AnchorHTMLAttributes<HTMLAnchorElement> & { newTabTitle?: string }) => (
    <a {...props}>{children}</a>
  ),
}));
vi.mock("../../common/actor-avatar", () => ({ ActorAvatar: () => <span /> }));
vi.mock("./custom-status-chip", () => ({ CustomStatusChip: () => <span /> }));
vi.mock("./status-icon", () => ({ StatusIcon: () => <span /> }));

function node(
  tempId: string,
  identifier: string,
  title: string,
  wave: number,
  state: string,
  criteria: string[],
): DependencyGraphNode {
  return {
    id: `node-${tempId}`,
    temp_id: tempId,
    issue_id: `issue-${tempId}`,
    issue: { id: `issue-${tempId}`, identifier, title, status: state } as unknown as DependencyGraphNode["issue"],
    title,
    description: "",
    acceptance_criteria: criteria,
    context: {},
    outputs: [],
    owner_type: null,
    owner_id: null,
    executor_type: null,
    executor_id: null,
    candidate_executors: [],
    reviewer_type: null,
    reviewer_id: null,
    runtime_id: null,
    model_id: null,
    wave,
    status: state,
    readiness: {
      state: state as DependencyGraphNode["readiness"]["state"],
      gate_open: state === "ready" || state === "done",
      satisfied_prerequisites: state === "done" ? 1 : 0,
      total_prerequisites: 1,
      unlock_condition: state === "blocked" ? "Waiting for prerequisite" : "Ready",
    },
  };
}

function edge(id: string, from: string, satisfied: boolean): DependencyGraphEdge {
  return {
    id,
    plan_id: "plan-1",
    from_issue_id: `issue-${from}`,
    to_issue_id: "issue-three",
    from,
    to: "three",
    type: "hard",
    reason: "The prerequisite output is required",
    consumed_output: "Completed work",
    prerequisite_status: satisfied ? "done" : "blocked",
    satisfied,
    satisfied_prerequisites: satisfied ? 1 : 0,
    total_prerequisites: 2,
    unlock_condition: "Both prerequisites must be satisfied",
  };
}

const graph = {
  plan: { id: "plan-1" },
  nodes: [
    node("one", "PB-1", "First task", 0, "done", ["First task is complete"]),
    node("two", "PB-2", "Second task", 0, "blocked", ["Second task is reviewed"]),
    node("three", "PB-3", "Release task", 1, "blocked", ["Both prerequisite outputs are present"]),
  ],
  edges: [edge("edge-one", "one", true), edge("edge-two", "two", false)],
  readiness: { total: 3, ready: 0, running: 0, blocked: 2, done: 1, cancelled: 0 },
} as unknown as DependencyGraphResponse;

describe("DependencyGraphView runtime surface", () => {
  it("renders both dependency edges and exposes acceptance criteria after selecting a task", () => {
    queryState.current.data = [graph];
    renderWithI18n(<DependencyGraphView />);

    expect(screen.getByRole("button", { name: /PB-1 to PB-3.*Satisfied/i })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /PB-2 to PB-3.*Blocked/i })).toBeInTheDocument();
    expect(screen.getAllByTitle("1 acceptance criteria")).toHaveLength(3);

    const targetNode = screen.getByLabelText(/PB-3 Release task/);
    fireEvent.click(targetNode);

    expect(screen.getByText("Both prerequisite outputs are present")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Open PB-3" })).toHaveAttribute("href", "/acme/issues/PB-3");
  });

  it("shows the dependent task gate as blocked until every prerequisite is satisfied", () => {
    queryState.current.data = [graph];
    renderWithI18n(<DependencyGraphView />);

    fireEvent.click(screen.getByRole("button", { name: /PB-1 to PB-3.*Satisfied/i }));

    expect(screen.getByText("Blocked — dependent task is still locked")).toBeInTheDocument();
    expect(screen.getByText("Both prerequisites must be satisfied")).toBeInTheDocument();
  });
});
