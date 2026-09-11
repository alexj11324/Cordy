// @vitest-environment jsdom

import { describe, expect, it, vi } from "vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, screen } from "@testing-library/react";
import type { Agent, AgentRuntime } from "@orvilo/core/types";
import { renderWithI18n } from "../../test/i18n";
import { NavigationProvider, type NavigationAdapter } from "../../navigation";
import { AgentTable } from "./agent-table";
import type { AgentListRow } from "./agents-page";
import {
  getColumns,
  getRowActionState,
} from "@orvilo/ui/components/blocks/data-grid-base-1/components/columns";
import type { IEmployee } from "@orvilo/ui/components/blocks/data-grid-base-1/components/data-grid-view";

vi.mock("../../runtimes/components/provider-logo", () => ({
  ProviderLogo: ({ provider }: { provider: string }) => (
    <span data-testid={`provider-logo-${provider}`}>{provider}</span>
  ),
}));
vi.mock("@orvilo/core/runtimes", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@orvilo/core/runtimes")>();
  return {
    ...actual,
    deviceKind: () => "laptop",
  };
});
vi.mock("@orvilo/core/hooks", () => ({
  useWorkspaceId: () => "ws-1",
}));
vi.mock("@orvilo/core/paths", () => ({
  useWorkspacePaths: () => ({
    newAgent: () => "/acme/agents/new",
    agentDetail: (id: string) => `/acme/agents/${id}`,
  }),
}));
vi.mock("@orvilo/core/api", () => ({
  api: {
    archiveAgent: vi.fn(),
    restoreAgent: vi.fn(),
    getBaseUrl: () => "",
  },
}));
vi.mock("@orvilo/core/workspace/queries", () => ({
  workspaceKeys: { agents: (wsId: string) => ["agents", wsId] },
}));

function agent(id: string, name: string): Agent {
  return {
    id,
    name,
    description: `${name}@example.com`,
    model: "sonnet",
    runtime_id: "rt-1",
    owner_id: "u-1",
    visibility: "workspace",
    permission_mode: "public_to",
    invocation_targets: [],
    archived_at: null,
    created_at: "2026-01-01T00:00:00Z",
    avatar_url: null,
  } as unknown as Agent;
}

function runtime(overrides: Partial<AgentRuntime> = {}): AgentRuntime {
  return {
    id: "rt-1",
    workspace_id: "ws-1",
    daemon_id: null,
    name: "Codex (Alex MacBook Pro)",
    runtime_mode: "local",
    provider: "codex",
    launch_header: "",
    status: "online",
    device_info: "Alex MacBook Pro · arm64 macOS",
    metadata: {},
    owner_id: "u-1",
    visibility: "private",
    last_seen_at: null,
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
    ...overrides,
  };
}

function row(
  id: string,
  name: string,
  overrides: Partial<AgentListRow> = {},
): AgentListRow {
  return {
    agent: agent(id, name),
    runtime: runtime(),
    presence: {
      availability: "online",
      workload: "idle",
      runningCount: 0,
      queuedCount: 0,
      capacity: 1,
    },
    activity: null,
    runCount: 12,
    lastActiveDays: 0,
    owner: {
      id: "mem-1",
      workspace_id: "ws-1",
      user_id: "u-1",
      role: "owner",
      created_at: "2026-01-01T00:00:00Z",
      name: "Mira Stone",
      email: "mira@example.com",
      avatar_url: null,
    },
    isOwnedByMe: false,
    canManage: true,
    ...overrides,
  };
}

const ROWS = [
  row("a-1", "Alpha Agent"),
  row("a-2", "Beta Agent", {
    agent: agent("a-2", "Beta Agent"),
    runtime: null,
    presence: null,
    runCount: 0,
    owner: null,
  }),
];

function makeAdapter(overrides: Partial<NavigationAdapter> = {}): NavigationAdapter {
  return {
    push: vi.fn(),
    replace: vi.fn(),
    back: vi.fn(),
    pathname: "/acme/agents",
    searchParams: new URLSearchParams(),
    hash: "",
    getShareableUrl: (p) => p,
    ...overrides,
  };
}

function renderTable({
  rows = ROWS,
  adapter = makeAdapter(),
  selectedIds = new Set<string>(),
  onSelectedIdsChange = vi.fn(),
  locale = "en",
  localDaemonId,
  localMachineName,
  currentUserId,
}: {
  rows?: AgentListRow[];
  adapter?: NavigationAdapter;
  selectedIds?: Set<string>;
  onSelectedIdsChange?: (ids: ReadonlySet<string>) => void;
  locale?: "en" | "zh-Hans";
  localDaemonId?: string | null;
  localMachineName?: string | null;
  currentUserId?: string | null;
} = {}) {
  // Base UI's scroll area settles its viewport with getAnimations(); keep the
  // jsdom shim local to this ReUI table test instead of changing every view.
  if (typeof Element.prototype.getAnimations !== "function") {
    Element.prototype.getAnimations = () => [];
  }
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  const buildUi = (nextRows: AgentListRow[]) => (
    <QueryClientProvider client={client}>
      <NavigationProvider value={adapter}>
        <AgentTable
          rows={nextRows}
          selectedIds={selectedIds}
          onSelectedIdsChange={onSelectedIdsChange}
          noMatchText="No agents"
          locale={locale}
          localDaemonId={localDaemonId}
          localMachineName={localMachineName}
          currentUserId={currentUserId}
        />
      </NavigationProvider>
    </QueryClientProvider>
  );
  const view = renderWithI18n(buildUi(rows), { locale });
  return {
    ...view,
    rerenderRows: (nextRows: AgentListRow[]) =>
      view.rerender(buildUi(nextRows)),
  };
}

describe("AgentTable uses data-grid-base-1", () => {
  it("renders the block chrome and agent rows", () => {
    renderTable();

    expect(screen.getByText("Agents")).toBeInTheDocument();
    expect(
      screen.getByPlaceholderText("Search agents…"),
    ).toBeInTheDocument();
    expect(screen.getByText("Alpha Agent")).toBeInTheDocument();
    expect(screen.getByText("Beta Agent")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "New agent" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Filter by status" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Agent actions" })).not.toBeInTheDocument();
    expect(screen.getByText("Device")).toBeInTheDocument();
    expect(screen.getByText("Owner")).toBeInTheDocument();
    expect(screen.getByText("Runs")).toBeInTheDocument();
    expect(screen.queryByText("Created")).not.toBeInTheDocument();
    expect(screen.queryByText("$0.00")).not.toBeInTheDocument();
    expect(screen.queryByText("$12.00")).not.toBeInTheDocument();
  });

  it("wires harness logo, device icon, owner avatar, and integer runs", () => {
    renderTable();

    expect(screen.getByTestId("provider-logo-codex")).toBeInTheDocument();
    expect(screen.getByText("Alex MacBook Pro")).toBeInTheDocument();
    expect(screen.queryByText("This machine")).not.toBeInTheDocument();
    expect(document.querySelector('[data-kind="laptop"]')).not.toBeNull();
    expect(screen.getByText("Mira Stone")).toBeInTheDocument();
    expect(screen.getByText("12")).toBeInTheDocument();
    expect(screen.getByText("Needs a device")).toBeInTheDocument();
    expect(document.querySelector('[data-kind="desktop"]')).toBeNull();
    expect(screen.getByText("0")).toBeInTheDocument();

    const ownerAvatar = screen.getByText("MS").closest('[data-slot="avatar"]');
    expect(ownerAvatar?.className).toMatch(/size-8/);

    expect(screen.queryByText("Offline")).not.toBeInTheDocument();
    expect(
      screen.queryByRole("columnheader", { name: "Status" }),
    ).not.toBeInTheDocument();
  });

  it("keeps a row actions menu", () => {
    renderTable();
    expect(screen.getAllByRole("button", { name: "Row actions" }).length).toBeGreaterThanOrEqual(
      2,
    );
  });

  it("selects a row from the checkbox without navigating", () => {
    const push = vi.fn();
    const onSelectedIdsChange = vi.fn();
    renderTable({
      adapter: makeAdapter({ push }),
      onSelectedIdsChange,
    });

    fireEvent.click(screen.getAllByRole("checkbox")[1]!);

    expect(onSelectedIdsChange).toHaveBeenCalled();
    expect(push).not.toHaveBeenCalled();
  });

  it("keeps pinyin matching in the ReUI table search", () => {
    renderTable({
      rows: [row("zh-1", "李云龙"), row("en-1", "Other Agent")],
    });

    fireEvent.change(screen.getByRole("textbox", { name: "Search agents…" }), {
      target: { value: "lyl" },
    });

    expect(screen.getByText("李云龙")).toBeInTheDocument();
    expect(screen.queryByText("Other Agent")).not.toBeInTheDocument();
  });

  it("localizes column menus and selection labels", () => {
    renderTable({ locale: "zh-Hans" });

    fireEvent.click(screen.getByRole("button", { name: "智能体" }));

    expect(screen.getByText("升序排列")).toBeInTheDocument();
    expect(screen.getByText("降序排列")).toBeInTheDocument();
    expect(screen.getByText("固定到左侧")).toBeInTheDocument();
    expect(screen.getByText("列")).toBeInTheDocument();
    expect(screen.getByRole("checkbox", { name: "全选" })).toBeInTheDocument();
  });

  it("derives lifecycle actions from row ownership and state", () => {
    const actions = { onDelete: vi.fn(), onRestore: vi.fn() };

    expect(
      getRowActionState(
        { canManage: false, isArchived: false, isSystemAgent: false },
        actions,
      ),
    ).toEqual({ canEdit: false, canArchive: false, canRestore: false });
    expect(
      getRowActionState(
        { canManage: true, isArchived: true, isSystemAgent: false },
        actions,
      ),
    ).toEqual({ canEdit: true, canArchive: false, canRestore: true });
    expect(
      getRowActionState(
        { canManage: true, isArchived: false, isSystemAgent: true },
        actions,
      ),
    ).toEqual({ canEdit: true, canArchive: false, canRestore: false });
    expect(
      getRowActionState(
        { canManage: true, isArchived: false, isSystemAgent: false },
        actions,
      ),
    ).toEqual({ canEdit: true, canArchive: true, canRestore: false });
  });

  it("sorts the Created column by the source timestamp", () => {
    const joined = getColumns().find((column) => column.id === "joined");
    const newer = {
      joined: "9/1/2026",
      joinedTimestamp: Date.parse("2026-09-01"),
    } as IEmployee;
    const older = {
      joined: "10/1/2025",
      joinedTimestamp: Date.parse("2025-10-01"),
    } as IEmployee;

    if (!joined || !("accessorFn" in joined) || typeof joined.accessorFn !== "function") {
      throw new Error("Created column must sort through its source timestamp accessor");
    }

    expect(joined.accessorFn(newer, 0)).toBeGreaterThan(
      joined.accessorFn(older, 1) as number,
    );
  });

  it("resets pagination when the supplied rows shrink", async () => {
    const rows = Array.from({ length: 6 }, (_, index) =>
      row(`agent-${index}`, `Agent ${index}`),
    );
    const view = renderTable({ rows });

    fireEvent.click(
      screen.getByRole("button", { name: "Go to next page" }),
    );
    expect(screen.getByText("Agent 5")).toBeInTheDocument();

    view.rerenderRows([rows[0]!]);

    expect(await screen.findByText("Agent 0")).toBeInTheDocument();
  });

  it("labels this machine after a new app mints a different daemon UUID", () => {
    renderTable({
      rows: [
        row("a-1", "Alpha Agent", {
          runtime: runtime({
            daemon_id: "daemon-old",
            owner_id: "u-1",
          }),
        }),
      ],
      localDaemonId: "daemon-new",
      localMachineName: "Alex MacBook Pro",
      currentUserId: "u-1",
    });

    expect(screen.getByText("This machine")).toBeInTheDocument();
    expect(screen.queryByText("Alex MacBook Pro")).not.toBeInTheDocument();
  });
});
