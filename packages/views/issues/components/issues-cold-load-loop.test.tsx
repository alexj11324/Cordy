/**
 * @vitest-environment jsdom
 *
 * PB-4985 regression — cold-load render loop on the Issues route.
 *
 * These tests render Board and Swimlane with the REAL react-virtuoso and the
 * REAL `useActorName`, while the member/agent/team directory queries are held
 * pending (the cold-load state). Before the fix, `useActorName` returned a
 * fresh `getActorName` on every render, which churned BoardView's `groups` /
 * SwimLaneView's `laneGroups`, re-fired the column-resync effect without end,
 * and react-virtuoso escalated it into "Maximum update depth exceeded". A
 * looping render never settles, so each test would hang/throw; the fix lets it
 * paint. (Unlike the sibling swimlane-view.test.tsx, this file intentionally
 * does NOT mock react-virtuoso or useActorName — those two reals are the whole
 * point of the reproduction.)
 */
import { describe, it, expect, vi, beforeEach } from "vitest";
import { act, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { BoardView } from "./board-view";
import { SwimLaneView } from "./swimlane-view";
import { IssueContextMenuProvider } from "../actions";
import { setApiInstance } from "@patchbay/core/api";
import type { ApiClient } from "@patchbay/core/api/client";
import type { Issue } from "@patchbay/core/types";
import type { IssueStatusPagination } from "../surface/use-issue-status-branches";
import { I18nProvider } from "@patchbay/core/i18n/react";
import enCommon from "../../locales/en/common.json";
import enIssues from "../../locales/en/issues.json";

const TEST_RESOURCES = { en: { common: enCommon, issues: enIssues } };

vi.mock("@patchbay/core/hooks", () => ({
  useWorkspaceId: () => "ws-1",
}));

vi.mock("@patchbay/core/paths", async () => {
  const actual = await vi.importActual<typeof import("@patchbay/core/paths")>(
    "@patchbay/core/paths",
  );
  return {
    ...actual,
    useWorkspaceSlug: () => "acme",
    useRequiredWorkspaceSlug: () => "acme",
    useWorkspacePaths: () => actual.paths.workspace("acme"),
  };
});

const mockAuthUser = { id: "user-1", email: "test@test.com", name: "Test User" };
vi.mock("@patchbay/core/auth", () => ({
  useAuthStore: Object.assign(
    (selector?: any) => {
      const state = { user: mockAuthUser, isAuthenticated: true };
      return selector ? selector(state) : state;
    },
    { getState: () => ({ user: mockAuthUser, isAuthenticated: true }) },
  ),
  registerAuthStore: vi.fn(),
  createAuthStore: vi.fn(),
}));

vi.mock("../../navigation", () => ({
  AppLink: ({ children, href, ...props }: any) => (
    <a href={href} {...props}>
      {children}
    </a>
  ),
  useNavigation: () => ({ push: vi.fn(), pathname: "/issues" }),
  resolveClickIntent: () => "push",
  useIntentNavigate: () => () => {},
  NavigationProvider: ({ children }: { children: React.ReactNode }) => children,
}));

vi.mock("@patchbay/core/issues/config", () => ({
  ALL_STATUSES: ["backlog", "todo", "in_progress", "in_review", "done", "blocked", "cancelled"],
  STATUS_ORDER: ["backlog", "todo", "in_progress", "in_review", "done", "blocked", "cancelled"],
  STATUS_CONFIG: {
    backlog: { label: "Backlog", iconColor: "text-muted-foreground", hoverBg: "hover:bg-accent" },
    todo: { label: "Todo", iconColor: "text-muted-foreground", hoverBg: "hover:bg-accent" },
    in_progress: { label: "In Progress", iconColor: "text-warning", hoverBg: "hover:bg-warning/10" },
    in_review: { label: "In Review", iconColor: "text-success", hoverBg: "hover:bg-success/10" },
    done: { label: "Done", iconColor: "text-info", hoverBg: "hover:bg-info/10" },
    blocked: { label: "Blocked", iconColor: "text-destructive", hoverBg: "hover:bg-destructive/10" },
    cancelled: { label: "Cancelled", iconColor: "text-muted-foreground", hoverBg: "hover:bg-accent" },
  },
  PRIORITY_ORDER: ["urgent", "high", "medium", "low", "none"],
  PRIORITY_DISPLAY_ORDER: ["none", "urgent", "high", "medium", "low"],
  PRIORITY_CONFIG: {
    urgent: { label: "Urgent", bars: 4, color: "text-destructive" },
    high: { label: "High", bars: 3, color: "text-warning" },
    medium: { label: "Medium", bars: 2, color: "text-warning" },
    low: { label: "Low", bars: 1, color: "text-info" },
    none: { label: "No priority", bars: 0, color: "text-muted-foreground" },
  },
}));

vi.mock("@patchbay/core/issues/mutations", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@patchbay/core/issues/mutations")>();
  return {
    ...actual,
  };
});

vi.mock("@patchbay/core/properties", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@patchbay/core/properties")>();
  return {
    ...actual,
    useSetIssueProperty: () => ({ mutate: vi.fn(), mutateAsync: vi.fn() }),
    useUnsetIssueProperty: () => ({ mutate: vi.fn(), mutateAsync: vi.fn() }),
  };
});

// Board default grouping is "status"; swimlane switches to "executor" per test.
const mockViewState: Record<string, unknown> = {
  grouping: "status",
  sortBy: "position",
  sortDirection: "asc",
  cardProperties: { priority: true, executor: true, dueDate: true, project: true, childProgress: true, labels: true },
  swimlaneGrouping: "executor",
  swimlaneOrders: { parent: [], project: [], executor: [] },
  collapsedSwimlanes: { parent: [], project: [], executor: [] },
  setSwimlaneGrouping: vi.fn(),
  setSwimlaneOrder: vi.fn(),
  toggleSwimlaneCollapsed: vi.fn(),
  hideStatus: vi.fn(),
  showStatus: vi.fn(),
  priorityFilters: [],
  executorFilters: [],
  includeNoExecutor: false,
  creatorFilters: [],
  projectFilters: [],
  includeNoProject: false,
  labelFilters: [],
  propertyFilters: {},
  cardPropertyIds: [],
  agentRunningFilter: false,
};
vi.mock("@patchbay/core/issues/stores/view-store-context", () => ({
  ViewStoreProvider: ({ children }: { children: ReactNode }) => children,
  useViewStore: (selector?: any) => (selector ? selector(mockViewState) : mockViewState),
  useViewStoreApi: () => ({ getState: () => mockViewState, setState: vi.fn(), subscribe: vi.fn() }),
}));

vi.mock("@patchbay/core/modals", () => ({
  useModalStore: Object.assign(
    () => ({ open: vi.fn() }),
    { getState: () => ({ open: vi.fn() }) },
  ),
}));

vi.mock("@dnd-kit/core", () => ({
  DndContext: ({
    children,
    onDragStart,
    onDragOver,
    onDragEnd,
  }: any) => {
    lastOnDragStart = onDragStart;
    lastOnDragOver = onDragOver;
    lastOnDragEnd = onDragEnd;
    return children;
  },
  DragOverlay: () => null,
  PointerSensor: class {},
  useSensor: () => ({}),
  useSensors: () => [],
  useDroppable: () => ({ setNodeRef: vi.fn(), isOver: false }),
  pointerWithin: vi.fn(),
  closestCenter: vi.fn(),
}));

vi.mock("@dnd-kit/sortable", () => ({
  SortableContext: ({ children }: any) => children,
  verticalListSortingStrategy: {},
  arrayMove: <T,>(arr: T[]): T[] => arr.slice(),
  useSortable: () => ({
    attributes: {},
    listeners: {},
    setNodeRef: vi.fn(),
    transform: null,
    transition: null,
    isDragging: false,
  }),
}));

vi.mock("@dnd-kit/utilities", () => ({
  CSS: { Transform: { toString: () => undefined } },
}));

// The whole point: directory queries stay pending so useActorName renders in
// the cold-load state. A never-resolving promise keeps `data` undefined.
const pending = () => new Promise<never>(() => {});

let lastOnDragStart: ((event: any) => void) | undefined;
let lastOnDragOver: ((event: any) => void) | undefined;
let lastOnDragEnd: ((event: any) => void) | undefined;

function page(total: number) {
  return {
    total,
    loaded: total,
    hasMore: false,
    isLoading: false,
    isFetching: false,
    isError: false,
    loadMore: vi.fn(),
    retry: vi.fn(),
  };
}

function makeIssue(overrides: Partial<Issue> & { id: string }): Issue {
  return {
    workspace_id: "ws-1",
    number: 1,
    identifier: `PROJ-${overrides.id}`,
    title: `Issue ${overrides.id}`,
    description: null,
    status: "todo",
    priority: "none",
    owner_type: null,
    owner_id: null,
    executor_type: null,
    executor_id: null,
    reviewer_type: null,
    reviewer_id: null,
    creator_type: "member",
    creator_id: "user-1",
    parent_issue_id: null,
    project_id: null,
    position: 100,
    stage: null,
    start_date: null,
    due_date: null,
    metadata: {},
    properties: {},
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
    ...overrides,
  };
}

function renderWithProviders(ui: ReactNode) {
  const qc = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 } },
  });
  return render(
    <QueryClientProvider client={qc}>
      <I18nProvider resources={TEST_RESOURCES} locale="en">
        <IssueContextMenuProvider>{ui}</IssueContextMenuProvider>
      </I18nProvider>
    </QueryClientProvider>,
  );
}

describe("Issues cold-load render loop (PB-4985)", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    lastOnDragStart = undefined;
    lastOnDragOver = undefined;
    lastOnDragEnd = undefined;
    mockViewState.grouping = "status";
    mockViewState.swimlaneGrouping = "executor";
    setApiInstance({
      listMembers: pending,
      listAgents: pending,
      listTeams: pending,
      getAgentTaskSnapshot: () => Promise.resolve([]),
      listChildrenByParents: () => Promise.resolve({ issues: [] }),
      listProjects: pending,
      // ActorAvatar resolves image URLs against the API base.
      getBaseUrl: () => "",
    } as unknown as ApiClient);
  });

  it("Board with a large column paints during cold load (real Virtuoso mounts, no update-depth loop)", async () => {
    // > BOARD_VIRTUALIZE_THRESHOLD (30) issues in one status column so a real
    // <Virtuoso> mounts (VirtuosoSeed is used below the threshold and cannot
    // reproduce the store-driven loop).
    const issues = Array.from({ length: 40 }, (_, i) =>
      makeIssue({ id: `b${i}`, title: `Board Card ${i}`, status: "todo", position: 100 + i }),
    );

    renderWithProviders(
      <BoardView
        issues={issues}
        visibleStatuses={["todo", "in_progress", "done"]}
        hiddenStatuses={[]}
        onMoveIssue={vi.fn()}
      />,
    );

    // Reaching a stable paint (column header visible) proves the render settled
    // instead of looping.
    await waitFor(() => {
      expect(screen.getByText("Todo")).toBeInTheDocument();
    });
    expect(screen.getByText("Board Card 0")).toBeInTheDocument();
  });

  it("Swimlane grouped by executor paints during cold load (real Virtuoso mounts, no update-depth loop)", async () => {
    mockViewState.swimlaneGrouping = "executor";
    const issues = [
      makeIssue({ id: "s1", title: "Swim Card 1", owner_type: "member", owner_id: "user-1", status: "todo" }),
      makeIssue({ id: "s2", title: "Swim Card 2", executor_type: "agent", executor_id: "agent-1", status: "in_progress" }),
      makeIssue({ id: "s3", title: "Swim Card 3", executor_type: null, executor_id: null, status: "todo" }),
    ];

    renderWithProviders(
      <SwimLaneView issues={issues} onMoveIssue={vi.fn()} />,
    );

    await waitFor(() => {
      expect(screen.getByText("Swim Card 1")).toBeInTheDocument();
    });
    expect(screen.getByText("Swim Card 3")).toBeInTheDocument();
  });

  it("hides empty status columns while keeping them as hidden drop targets", () => {
    const issues = [makeIssue({ id: "todo-1", status: "todo" })];
    const pagination = {
      todo: page(1),
      in_progress: page(0),
    } as unknown as IssueStatusPagination;

    const { container } = renderWithProviders(
      <BoardView
        issues={issues}
        visibleStatuses={["todo", "in_progress"]}
        hiddenStatuses={[]}
        statusPagination={pagination}
        onMoveIssue={vi.fn()}
      />,
    );

    expect(
      container.querySelector('[data-board-column="status:todo"]'),
    ).not.toBeNull();
    expect(
      container.querySelector('[data-board-column="status:in_progress"]'),
    ).toBeNull();
    expect(
      container.querySelector('[data-hidden-column-drop-target="in_progress"]'),
    ).not.toBeNull();
  });

  it("does not make filter-only hidden statuses drop targets", () => {
    const issues = [makeIssue({ id: "todo-1", status: "todo" })];

    const { container } = renderWithProviders(
      <BoardView
        issues={issues}
        visibleStatuses={["todo"]}
        hiddenStatuses={["in_progress"]}
        droppableHiddenStatuses={[]}
        onMoveIssue={vi.fn()}
      />,
    );

    expect(
      container.querySelector('[data-hidden-column-drop-target="in_progress"]'),
    ).toBeNull();
  });

  it("reveals an auto-hidden empty status after a card is dropped there", () => {
    const onMoveIssue = vi.fn();
    const issues = [makeIssue({ id: "todo-1", status: "todo" })];
    const pagination = {
      todo: page(1),
      in_progress: page(0),
    } as unknown as IssueStatusPagination;

    const { container } = renderWithProviders(
      <BoardView
        issues={issues}
        visibleStatuses={["todo", "in_progress"]}
        hiddenStatuses={[]}
        statusPagination={pagination}
        onMoveIssue={onMoveIssue}
      />,
    );

    expect(
      container.querySelector('[data-board-column="status:in_progress"]'),
    ).toBeNull();

    act(() => {
      lastOnDragStart?.({ active: { id: "todo-1" } });
    });
    act(() => {
      lastOnDragOver?.({
        active: { id: "todo-1" },
        over: { id: "status:in_progress" },
      });
    });
    act(() => {
      lastOnDragEnd?.({
        active: { id: "todo-1" },
        over: { id: "status:in_progress" },
      });
    });

    expect(onMoveIssue).toHaveBeenCalledWith(
      "todo-1",
      expect.objectContaining({ status: "in_progress" }),
      expect.objectContaining({
        onSettled: expect.any(Function),
        onSuccess: expect.any(Function),
        onError: expect.any(Function),
      }),
    );
    expect(
      container.querySelector('[data-board-column="status:in_progress"]'),
    ).not.toBeNull();
    expect(
      container.querySelector('[data-hidden-column-drop-target="in_progress"]'),
    ).toBeNull();
  });

  it("moves a card to a hidden status and reveals that status", () => {
    const onMoveIssue = vi.fn();
    const issues = [makeIssue({ id: "todo-1", status: "todo" })];
    const showStatus = mockViewState.showStatus as ReturnType<typeof vi.fn>;
    const pagination = {
      todo: page(1),
      in_progress: page(0),
    } as unknown as IssueStatusPagination;

    renderWithProviders(
      <BoardView
        issues={issues}
        visibleStatuses={["todo"]}
        hiddenStatuses={["in_progress"]}
        statusPagination={pagination}
        onMoveIssue={onMoveIssue}
      />,
    );

    act(() => {
      lastOnDragStart?.({ active: { id: "todo-1" } });
    });
    act(() => {
      lastOnDragOver?.({
        active: { id: "todo-1" },
        over: { id: "status:in_progress" },
      });
    });
    act(() => {
      lastOnDragEnd?.({
        active: { id: "todo-1" },
        over: { id: "status:in_progress" },
      });
    });

    expect(onMoveIssue).toHaveBeenCalledWith(
      "todo-1",
      expect.objectContaining({ status: "in_progress" }),
      expect.objectContaining({
        onSettled: expect.any(Function),
        onSuccess: expect.any(Function),
        onError: expect.any(Function),
      }),
    );
    expect(showStatus).toHaveBeenCalledWith("in_progress");
  });
});
