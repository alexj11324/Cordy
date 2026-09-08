import { describe, expect, it, vi } from "vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, screen, within } from "@testing-library/react";
import type { Agent } from "@orvilo/core/types";
import { renderWithI18n } from "../../test/i18n";
import { NavigationProvider, type NavigationAdapter } from "../../navigation";
import { AgentTable } from "./agent-table";
import type { AgentListRow } from "./agents-page";

// Row navigation is delegated from the table's scroll container instead of
// being spread onto each <tr>, because the grid owns row rendering. These
// tests pin what that delegation must keep doing — plain click, middle click,
// and NOT firing when a control inside the row was the target — plus the two
// regressions the ListGrid version shipped twice: a user-disabled column must
// disappear from the header, and an enabled one must not.

// Identity chrome pulls workspace-scoped queries that say nothing about the
// behaviors under test.
vi.mock("../../common/actor-avatar", () => ({
  ActorAvatar: () => <span data-testid="avatar" />,
}));
vi.mock("./agent-row-actions", () => ({
  AgentRowActions: () => <span data-testid="row-actions" />,
}));

// The grid virtualizes; render every row so assertions see them all.
vi.mock("@tanstack/react-virtual", () => ({
  useVirtualizer: ({ count }: { count: number }) => ({
    getVirtualItems: () =>
      Array.from({ length: count }, (_, index) => ({
        index,
        key: index,
        start: index * 64,
        end: (index + 1) * 64,
        size: 64,
      })),
    getTotalSize: () => count * 64,
    measure: vi.fn(),
  }),
}));

function agent(id: string, name: string): Agent {
  return {
    id,
    name,
    description: "",
    model: "sonnet",
    runtime_id: "rt-1",
    owner_id: "u-1",
    visibility: "workspace",
    permission_mode: "public_to",
    invocation_targets: [],
    archived_at: null,
    created_at: "2026-01-01T00:00:00Z",
  } as unknown as Agent;
}

function row(id: string, name: string): AgentListRow {
  return {
    agent: agent(id, name),
    runtime: null,
    presence: null,
    activity: null,
    runCount: 0,
    lastActiveDays: null,
    owner: null,
    isOwnedByMe: false,
    canManage: true,
  };
}

const ROWS = [row("a-1", "Alpha Agent"), row("a-2", "Beta Agent")];

function makeAdapter(overrides: Partial<NavigationAdapter> = {}) {
  return {
    push: vi.fn(),
    replace: vi.fn(),
    back: vi.fn(),
    pathname: "/",
    searchParams: new URLSearchParams(),
    hash: "",
    getShareableUrl: (p: string) => p,
    ...overrides,
  } as NavigationAdapter;
}

function renderTable({
  adapter = makeAdapter(),
  selectedIds = new Set<string>(),
  onToggleSelected = vi.fn(),
  hidden = [] as string[],
}: {
  adapter?: NavigationAdapter;
  selectedIds?: Set<string>;
  onToggleSelected?: () => void;
  hidden?: string[];
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
        onToggleSelected={onToggleSelected}
        allSelected={false}
        someSelected={false}
        onToggleAll={vi.fn()}
        sort={{ field: "name", direction: "asc" }}
        onSort={vi.fn()}
        isColVisible={(key) => !hidden.includes(key)}
        duplicateHref={() => "/dup"}
        agentDetailHref={(id) => `/acme/agents/${id}`}
        noMatchText="No agents"
        locale="en"
      />
    </NavigationProvider>
    </QueryClientProvider>,
  );
}

function rowFor(name: string): HTMLElement {
  const cell = screen.getByText(name).closest("tr");
  if (!cell) throw new Error(`no row for ${name}`);
  return cell;
}

describe("AgentTable row navigation", () => {
  it("pushes the agent detail route on a plain row click", () => {
    const push = vi.fn();
    renderTable({ adapter: makeAdapter({ push }) });

    fireEvent.click(rowFor("Beta Agent"));

    expect(push).toHaveBeenCalledWith("/acme/agents/a-2");
  });

  it("opens a background tab on middle click", () => {
    const push = vi.fn();
    const openInNewTab = vi.fn();
    renderTable({ adapter: makeAdapter({ push, openInNewTab }) });

    rowFor("Alpha Agent").dispatchEvent(
      new MouseEvent("auxclick", { bubbles: true, button: 1, cancelable: true }),
    );

    expect(openInNewTab).toHaveBeenCalledWith("/acme/agents/a-1", "Alpha Agent");
    expect(push).not.toHaveBeenCalled();
  });

  it("prefetches the route once per row hovered", () => {
    const prefetch = vi.fn();
    renderTable({ adapter: makeAdapter({ prefetch }) });

    const beta = rowFor("Beta Agent");
    fireEvent.mouseOver(beta);
    fireEvent.mouseOver(within(beta).getByText("Beta Agent"));

    expect(prefetch).toHaveBeenCalledTimes(1);
    expect(prefetch).toHaveBeenCalledWith("/acme/agents/a-2");
  });

  it("selects without navigating when the row checkbox is clicked", () => {
    const push = vi.fn();
    const onToggleSelected = vi.fn();
    renderTable({ adapter: makeAdapter({ push }), onToggleSelected });

    const [checkbox] = within(rowFor("Alpha Agent")).getAllByRole("button");
    fireEvent.click(checkbox!);

    expect(onToggleSelected).toHaveBeenCalledWith("a-1");
    expect(push).not.toHaveBeenCalled();
  });
});

describe("AgentTable column visibility", () => {
  it("drops a column the user switched off and keeps the rest", () => {
    renderTable({ hidden: ["runs"] });

    expect(screen.queryByRole("columnheader", { name: "Runs" })).toBeNull();
    expect(
      screen.getByRole("columnheader", { name: "Model" }),
    ).toBeInTheDocument();
  });
});
