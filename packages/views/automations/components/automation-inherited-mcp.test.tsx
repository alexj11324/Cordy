import { QueryClient, QueryClientProvider, queryOptions } from "@tanstack/react-query";
import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { renderWithI18n } from "../../test/i18n";
import { AutomationInheritedMcp } from "./automation-inherited-mcp";
import type { AssigneeSelection } from "./pickers/agent-picker";

const mocks = vi.hoisted(() => ({ agents: vi.fn(), teams: vi.fn(), runtimes: vi.fn(), inventory: vi.fn() }));
vi.mock("@orvilo/core/hooks", () => ({ useWorkspaceId: () => "ws-test" }));
vi.mock("@orvilo/core/auth", () => ({
  useAuthStore: (selector: (state: { user: { id: string } }) => unknown) => selector({ user: { id: "user-1" } }),
}));
vi.mock("@orvilo/core/api", () => ({ api: {
  listAgents: mocks.agents, listTeams: mocks.teams, listRuntimes: mocks.runtimes,
} }));
vi.mock("@orvilo/core/runtimes", async (importOriginal) => ({
  ...await importOriginal<typeof import("@orvilo/core/runtimes")>(),
  runtimeCapabilitiesOptions: (runtimeId: string | null) => queryOptions({
    queryKey: ["runtime-capabilities", runtimeId],
    queryFn: () => mocks.inventory(runtimeId),
    enabled: !!runtimeId,
    retry: false,
  }),
}));

const runtime = { id: "runtime-1", runtime_mode: "local", owner_id: "user-1", visibility: "private", status: "online" };
const agent = { id: "agent-1", name: "Scout", runtime_id: "runtime-1" };

function renderInventory(assignee: AssigneeSelection | null, overriddenNames: string[] = []) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } });
  const ui = (next: AssigneeSelection | null) => (
    <QueryClientProvider client={client}>
      <AutomationInheritedMcp assignee={next} overriddenNames={overriddenNames} />
    </QueryClientProvider>
  );
  const rendered = renderWithI18n(ui(assignee));
  return { ...rendered, select: (next: AssigneeSelection) => rendered.rerender(ui(next)) };
}

describe("automation inherited MCP inventory", () => {
  beforeEach(() => {
    mocks.agents.mockReset().mockResolvedValue([agent]);
    mocks.teams.mockReset().mockResolvedValue([{ id: "team-1", leader_id: "agent-1" }]);
    mocks.runtimes.mockReset().mockResolvedValue([runtime]);
    mocks.inventory.mockReset().mockResolvedValue({ mcpSupported: true, mcpServers: [
      { name: "Disabled search", enabled: false, transport: "http", source: "User config" },
      { name: "Native docs", enabled: true, transport: "stdio", source: "User config" },
    ] });
  });

  it("asks for an executor before querying native capabilities", () => {
    renderInventory(null);
    expect(screen.getByText("Select an agent or team to see its available MCP servers.")).toBeInTheDocument();
    expect(mocks.agents).not.toHaveBeenCalled();
    expect(mocks.runtimes).not.toHaveBeenCalled();
    expect(mocks.inventory).not.toHaveBeenCalled();
  });

  it("shows runtime configuration names without pretending they are checkboxes or live connections", async () => {
    renderInventory({ type: "agent", id: "agent-1" });
    expect(await screen.findByText("Native docs")).toBeInTheDocument();
    expect(screen.getByText("Inherited")).toBeInTheDocument();
    expect(screen.getAllByRole("listitem")[0]).toHaveTextContent("Native docs");
    expect(screen.getByText("Disabled search")).toBeInTheDocument();
    expect(screen.getByText("Disabled in Harness")).toBeInTheDocument();
    expect(screen.queryByRole("checkbox")).not.toBeInTheDocument();
    expect(mocks.inventory).toHaveBeenCalledWith("runtime-1");
    expect(mocks.teams).not.toHaveBeenCalled();
  });

  it("resolves a team's native inventory from its leader", async () => {
    renderInventory({ type: "team", id: "team-1" });
    expect(await screen.findByText("Native docs")).toBeInTheDocument();
    expect(screen.getByText("MCP from Scout")).toBeInTheDocument();
    expect(mocks.teams).toHaveBeenCalledTimes(1);
    expect(mocks.inventory).toHaveBeenCalledWith("runtime-1");
  });

  it("switches inventory when the selected agent changes", async () => {
    mocks.agents.mockResolvedValue([agent, { id: "agent-2", name: "Writer", runtime_id: "runtime-2" }]);
    mocks.runtimes.mockResolvedValue([runtime, { ...runtime, id: "runtime-2" }]);
    mocks.inventory.mockImplementation(async (id: string) => ({
      mcpSupported: true, mcpServers: [{ name: id === "runtime-1" ? "Scout MCP" : "Writer MCP", enabled: true }],
    }));
    const view = renderInventory({ type: "agent", id: "agent-1" });
    await screen.findByText("Scout MCP");
    view.select({ type: "agent", id: "agent-2" });
    expect(await screen.findByText("Writer MCP")).toBeInTheDocument();
    expect(screen.queryByText("Scout MCP")).not.toBeInTheDocument();
  });

  it.each([
    [{ status: "offline" }, "This Harness is offline."],
    [{ owner_id: "someone-else" }, "You do not have access to this Harness's MCP inventory."],
    [{ runtime_mode: "cloud" }, "This Harness does not expose a native MCP inventory."],
  ])("does not discover inaccessible or unavailable runtimes: %j", async (overrides, message) => {
    mocks.runtimes.mockResolvedValue([{ ...runtime, ...overrides }]);
    renderInventory({ type: "agent", id: "agent-1" });
    expect(await screen.findByText(message)).toBeInTheDocument();
    expect(mocks.inventory).not.toHaveBeenCalled();
  });

  it("shows loading and allows retrying a failed inventory lookup", async () => {
    mocks.inventory.mockRejectedValueOnce(new Error("Daemon unavailable"));
    renderInventory({ type: "agent", id: "agent-1" });
    expect(screen.getByRole("status")).toHaveTextContent("Loading tools...");
    await screen.findByText("Could not load MCP servers");
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(await screen.findByText("Native docs")).toBeInTheDocument();
    expect(mocks.inventory).toHaveBeenCalledTimes(2);
  });

  it("reports unsupported discovery separately from an empty inventory", async () => {
    mocks.inventory.mockResolvedValue({ mcpSupported: false, mcpServers: [] });
    renderInventory({ type: "agent", id: "agent-1" });
    expect(await screen.findByText("This Harness does not expose a native MCP inventory.")).toBeInTheDocument();
    expect(screen.queryByText("No MCP servers found in this Harness's supported configuration.")).not.toBeInTheDocument();
  });

  it("marks a native name that is overridden by the effective workspace selection", async () => {
    renderInventory({ type: "agent", id: "agent-1" }, ["Native docs"]);
    await waitFor(() => expect(screen.getByText("Workspace override")).toBeInTheDocument());
    expect(screen.queryByText("Inherited")).not.toBeInTheDocument();
  });
});
