// @vitest-environment jsdom

import { describe, expect, it, vi } from "vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, screen } from "@testing-library/react";
import type { Agent, AgentRuntime } from "@orvilo/core/types";
import { renderWithI18n } from "../../test/i18n";
import { NavigationProvider, type NavigationAdapter } from "../../navigation";
import { AgentTable } from "./agent-table";
import type { AgentListRow } from "./agents-page";

vi.mock("../../runtimes/components/provider-logo", () => ({
  ProviderLogo: ({ provider }: { provider: string }) => (
    <span data-testid={`provider-logo-${provider}`}>{provider}</span>
  ),
}));
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
  api: { archiveAgent: vi.fn(), getBaseUrl: () => "" },
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
  adapter = makeAdapter(),
  selectedIds = new Set<string>(),
  onSelectedIdsChange = vi.fn(),
}: {
  adapter?: NavigationAdapter;
  selectedIds?: Set<string>;
  onSelectedIdsChange?: (ids: ReadonlySet<string>) => void;
} = {}) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return renderWithI18n(
    <QueryClientProvider client={client}>
      <NavigationProvider value={adapter}>
        <AgentTable
          rows={ROWS}
          selectedIds={selectedIds}
          onSelectedIdsChange={onSelectedIdsChange}
          noMatchText="No agents"
          locale="en"
        />
      </NavigationProvider>
    </QueryClientProvider>,
  );
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
    expect(
      screen.getByRole("button", { name: "Agent actions" }),
    ).toBeInTheDocument();
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
});
