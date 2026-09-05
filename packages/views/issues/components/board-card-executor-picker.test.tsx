import { afterEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import type { Issue, IssueExecutorType } from "@patchbay/core/types";
import { AppLink, NavigationProvider, type NavigationAdapter } from "../../navigation";
import {
  IssueSurfaceActionsProvider,
  type IssueSurfaceActions,
} from "../surface/actions-context";
import { BoardCardContent } from "./board-card";

vi.mock("@tanstack/react-query", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@tanstack/react-query")>()),
  useQuery: () => ({ data: [] }),
}));

vi.mock("@patchbay/core/hooks", () => ({
  useWorkspaceId: () => "ws-1",
}));

vi.mock("@patchbay/core/properties", () => ({
  propertyListOptions: () => ({ queryKey: ["properties"] }),
}));

vi.mock("@patchbay/core/auth", () => ({
  useAuthStore: (selector: (state: { user: { id: string } }) => unknown) =>
    selector({ user: { id: "viewer-1" } }),
}));

vi.mock("@patchbay/core/agents", () => ({
  isAgentRuntimeBound: () => true,
  useAgentPresenceDetail: () => ({ availability: "offline", workload: null }),
}));

vi.mock("@patchbay/core/paths", () => ({
  useCurrentWorkspace: () => ({ id: "ws-1", slug: "acme" }),
  useWorkspacePaths: () => ({
    memberDetail: (id: string) => `/acme/members/${id}`,
    agentDetail: (id: string) => `/acme/agents/${id}`,
    teamDetail: (id: string) => `/acme/teams/${id}`,
  }),
}));

const viewState = vi.hoisted(() => ({
  cardProperties: {
    priority: false,
    description: false,
    executor: true,
    startDate: true,
    dueDate: true,
    project: false,
    childProgress: false,
    labels: false,
  },
  cardPropertyIds: [],
}));

vi.mock("@patchbay/core/issues/stores/view-store-context", () => ({
  useViewStore: (selector: (state: typeof viewState) => unknown) => selector(viewState),
}));

vi.mock("@patchbay/core/workspace/hooks", () => ({
  useActorName: () => ({
    getActorName: (type: string) => `Assigned ${type}`,
    getActorInitials: () => "AA",
    getActorAvatarUrl: () => null,
  }),
}));

vi.mock("../../i18n", () => ({
  useLocale: () => "en",
  useT: () => ({
    t: (selector: (dict: Record<string, unknown>) => unknown) => {
      const path: string[] = [];
      const proxy: Record<string, unknown> = new Proxy(
        {},
        {
          get: (_target, prop: string) => {
            path.push(prop);
            return proxy;
          },
        },
      );
      selector(proxy);
      return path.join(".");
    },
  }),
  useTimeAgo: () => () => "now",
}));

vi.mock("../../agents/components/agent-profile-card", () => ({
  AgentProfileCard: () => null,
}));

vi.mock("../../agents/components/agent-live-peek-card", () => ({
  AgentLivePeekCard: () => null,
}));

vi.mock("../../members/member-profile-card", () => ({
  MemberProfileCard: () => null,
}));

vi.mock("../../teams/components/team-profile-card", () => ({
  TeamProfileCard: () => null,
}));

vi.mock("./issue-agent-activity-indicator", () => ({
  IssueAgentActivityIndicator: () => null,
}));

const navigation: NavigationAdapter = {
  push: vi.fn(),
  replace: vi.fn(),
  back: vi.fn(),
  pathname: "/acme/issues",
  searchParams: new URLSearchParams(),
  hash: "",
  getShareableUrl: (path) => `https://app.example${path}`,
};

const actions: IssueSurfaceActions = {
  isPending: false,
  createIssue: vi.fn(),
  updateIssue: vi.fn(),
  moveIssue: vi.fn(),
  batchUpdate: vi.fn().mockResolvedValue(undefined),
  batchDelete: vi.fn().mockResolvedValue(undefined),
};

function makeIssue(
  executorType: IssueExecutorType | null,
  extras: Partial<Issue> = {},
): Issue {
  return {
    id: `issue-${executorType ?? "none"}`,
    workspace_id: "ws-1",
    number: 6082,
    identifier: "MUL-6082",
    title: "Fix Board executor interaction",
    description: null,
    status: "todo",
    priority: "none",
    owner_type: null,
    owner_id: null,
    executor_type: executorType,
    executor_id: executorType ? `${executorType}-1` : null,
    reviewer_type: null,
    reviewer_id: null,
    creator_type: "member",
    creator_id: "member-1",
    parent_issue_id: null,
    project_id: null,
    position: 1,
    stage: null,
    start_date: "2026-08-12",
    due_date: "2026-08-13",
    metadata: {},
    properties: {},
    labels: [],
    created_at: "2026-08-12T00:00:00Z",
    updated_at: "2026-08-12T00:00:00Z",
    ...extras,
  };
}

const defaultCardProperties = { ...viewState.cardProperties };

function renderCard(issue: Issue) {
  return render(
    <NavigationProvider value={navigation}>
      <IssueSurfaceActionsProvider actions={actions}>
        <AppLink href={`/acme/issues/${issue.id}`}>
          <BoardCardContent issue={issue} editable />
        </AppLink>
      </IssueSurfaceActionsProvider>
    </NavigationProvider>,
  );
}

describe("BoardCardContent executor picker", () => {
  afterEach(() => {
    viewState.cardProperties = { ...defaultCardProperties };
    vi.clearAllMocks();
  });

  it.each<IssueExecutorType>(["agent", "team"])(
    "opens the picker from an avatar-only %s executor without navigating the card",
    (executorType) => {
      const issue = makeIssue(executorType);
      const { container } = renderCard(issue);
      const avatar = container.querySelector('[data-slot="avatar"]');

      expect(avatar).not.toBeNull();
      expect(avatar!.closest('[role="link"]')).toBeNull();
      expect(screen.queryByText("Assigned agent")).not.toBeInTheDocument();
      expect(screen.queryByText("Assigned team")).not.toBeInTheDocument();
      expect(fireEvent.click(avatar!)).toBe(false);
      expect(screen.getByRole("textbox")).toBeInTheDocument();
      expect(navigation.push).not.toHaveBeenCalled();
    },
  );

  it("does not render Unassigned copy on an empty executor slot", () => {
    const { container } = renderCard(makeIssue(null));

    expect(container.querySelector('[data-slot="avatar"]')).toBeNull();
    expect(
      screen.queryByText("pickers.executor.trigger_unassigned"),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByText("pickers.owner.trigger_unassigned"),
    ).not.toBeInTheDocument();
    expect(
      screen.getByLabelText("pickers.executor.trigger_unassigned"),
    ).toBeInTheDocument();
    expect(
      screen.getByLabelText("pickers.owner.trigger_unassigned"),
    ).toBeInTheDocument();
  });

  it("opens the picker from the unassigned hover target without navigating", () => {
    renderCard(makeIssue(null));

    fireEvent.click(screen.getByLabelText("pickers.executor.trigger_unassigned"));

    expect(screen.getByRole("textbox")).toBeInTheDocument();
    expect(navigation.push).not.toHaveBeenCalled();
  });

  it("renders board labels as a color dot instead of a filled pill", () => {
    viewState.cardProperties.labels = true;
    renderCard(
      makeIssue("agent", {
        labels: [
          {
            id: "label-1",
            workspace_id: "ws-1",
            name: "Feature",
            color: "#8b5cf6",
            created_at: "2026-08-12T00:00:00Z",
            updated_at: "2026-08-12T00:00:00Z",
          },
        ],
      }),
    );

    const chip = screen.getByLabelText("Feature");
    expect(chip).not.toHaveStyle({ backgroundColor: "rgb(139, 92, 246)" });
    expect(chip.querySelector('[aria-hidden="true"]')).toHaveStyle({
      backgroundColor: "rgb(139, 92, 246)",
    });
  });

  it("stacks owner and executor avatars in the identifier row", () => {
    const { container } = renderCard(
      makeIssue("agent", { owner_type: "member", owner_id: "member-1" }),
    );
    const stack = container.querySelector("[data-board-actor-stack]");
    const identifierRow = screen.getByText("MUL-6082").closest(".justify-between");
    if (!(stack instanceof HTMLElement) || !(identifierRow instanceof HTMLElement)) {
      throw new Error("expected owner/executor stack in the identifier row");
    }
    const avatars = stack.querySelectorAll('[data-slot="avatar"]');
    const slots = stack.querySelectorAll(":scope > span");

    expect(identifierRow).toContainElement(stack);
    expect(avatars).toHaveLength(2);
    expect(slots[1]).toHaveStyle({ marginLeft: "-8px" });
    expect(screen.queryByText("Assigned member")).not.toBeInTheDocument();
    expect(screen.queryByText("Assigned agent")).not.toBeInTheDocument();
  });

  it("opens the owner picker from the stacked owner avatar without navigating", () => {
    const { container } = renderCard(
      makeIssue("agent", { owner_type: "member", owner_id: "member-1" }),
    );
    const avatars = container.querySelectorAll('[data-slot="avatar"]');

    expect(fireEvent.click(avatars[0]!)).toBe(false);
    expect(
      screen.getByPlaceholderText("pickers.owner.search_placeholder"),
    ).toBeInTheDocument();
    expect(navigation.push).not.toHaveBeenCalled();
  });

  it("places priority on the chip row with labels, not next to the identifier", () => {
    viewState.cardProperties.priority = true;
    viewState.cardProperties.labels = true;
    const { container } = renderCard(
      makeIssue("agent", {
        priority: "high",
        labels: [
          {
            id: "label-1",
            workspace_id: "ws-1",
            name: "Feature",
            color: "#8b5cf6",
            created_at: "2026-08-12T00:00:00Z",
            updated_at: "2026-08-12T00:00:00Z",
          },
        ],
      }),
    );

    const chipRow = container.querySelector("[data-board-chip-row]");
    const identifierRow = screen.getByText("MUL-6082").closest(".justify-between");
    const priority = screen.getByLabelText("priority.high");
    const feature = screen.getByLabelText("Feature");
    if (!(chipRow instanceof HTMLElement) || !(identifierRow instanceof HTMLElement)) {
      throw new Error("expected identifier row and chip row");
    }

    expect(identifierRow).not.toContainElement(priority);
    expect(chipRow).toContainElement(priority);
    expect(chipRow).toContainElement(feature);
  });
});
