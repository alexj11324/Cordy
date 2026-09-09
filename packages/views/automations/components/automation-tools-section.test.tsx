import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ComponentProps } from "react";
import type { Automation } from "@orvilo/core/types";
import { renderWithI18n } from "../../test/i18n";
import type { AssigneeSelection } from "./pickers/agent-picker";

const mocks = vi.hoisted(() => ({
  update: vi.fn(),
  slackList: vi.fn(),
  slackCatalog: vi.fn(),
  mcpList: vi.fn(),
  inventory: vi.fn(),
}));

vi.mock("@orvilo/core/hooks", () => ({ useWorkspaceId: () => "workspace-1" }));
vi.mock("@orvilo/core/auth", () => ({
  useAuthStore: (selector: (state: { user: { id: string } }) => unknown) => selector({ user: { id: "user-1" } }),
}));
vi.mock("@orvilo/core/paths", () => ({
  useWorkspacePaths: () => ({ settings: () => "/acme/settings" }),
}));
vi.mock("@orvilo/core/automations/mutations", () => ({
  useUpdateAutomation: () => ({ mutate: mocks.update, isPending: false }),
}));
vi.mock("@orvilo/core/workspace/queries", () => ({
  workspaceMcpServersOptions: () => ({
    queryKey: ["mcp"],
    queryFn: () => mocks.mcpList(),
  }),
  agentListOptions: () => ({ queryKey: ["agents"], queryFn: async () => [{ id: "agent-1", name: "Scout", runtime_id: "runtime-1" }] }),
  teamListOptions: () => ({ queryKey: ["teams"], queryFn: async () => [] }),
}));
vi.mock("@orvilo/core/runtimes", async (importOriginal) => ({
  ...await importOriginal<typeof import("@orvilo/core/runtimes")>(),
  runtimeListOptions: () => ({ queryKey: ["runtimes"], queryFn: async () => [{ id: "runtime-1", runtime_mode: "local", status: "online", owner_id: "user-1", visibility: "public" }] }),
  runtimeCapabilitiesOptions: (id: string | null) => ({ queryKey: ["runtime-mcp", id], queryFn: () => mocks.inventory(id), enabled: !!id }),
}));
vi.mock("@orvilo/core/slack/queries", () => ({
  slackInstallationsOptions: () => ({
    queryKey: ["slack-installations"],
    queryFn: () => mocks.slackList(),
  }),
  slackAutomationCatalogOptions: () => ({
    queryKey: ["slack-catalog"],
    queryFn: () => mocks.slackCatalog(),
    enabled: true,
  }),
}));
vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn() } }));
vi.mock("../../navigation", () => ({
  AppLink: ({ href, ...props }: ComponentProps<"a">) => <a href={href} {...props} />,
}));

import { AutomationToolsSection } from "./automation-tools-section";

function automation(tools: Record<string, unknown>): Automation {
  return {
    id: "automation-1",
    workspace_id: "workspace-1",
    title: "Release notes",
    description: null,
    executor_type: "agent",
    executor_id: "agent-1",
    status: "paused",
    execution_mode: "run_only",
    issue_title_template: null,
    tools,
    created_by_type: "member",
    created_by_id: "member-1",
    last_run_at: null,
    created_at: "2026-09-06T20:00:00Z",
    updated_at: "2026-09-06T20:00:00Z",
  };
}

function renderSection(tools: Record<string, unknown>, assignee: AssigneeSelection | null = null) {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const result = renderWithI18n(
    <QueryClientProvider client={queryClient}>
      <AutomationToolsSection automation={automation(tools)} assignee={assignee} canWrite />
    </QueryClientProvider>,
  );
  return { ...result, queryClient };
}

describe("AutomationToolsSection built-in tools", () => {
  beforeEach(() => {
    mocks.update.mockReset();
    mocks.slackList.mockReset().mockResolvedValue({ installations: [] });
    mocks.slackCatalog.mockReset().mockResolvedValue({ channels: [] });
    mocks.mcpList.mockReset().mockResolvedValue([{ id: "mcp-1", name: "Issue tracker" }]);
    mocks.inventory.mockReset().mockResolvedValue({ mcpSupported: true, mcpServers: [{ name: "Native docs", enabled: true }] });
  });

  it("removes Memories config while preserving Slack and MCP settings", async () => {
    const user = userEvent.setup();
    renderSection({
      memories: { enabled: false },
      slack_send: { enabled: false, installation_id: "install-1", channel_ids: ["C1"] },
      mcp_server_ids: ["mcp-1"],
    });

    await user.click(await screen.findByRole("button", { name: "Remove Memories" }));
    expect(mocks.update).toHaveBeenCalledWith({
      id: "automation-1",
      tools: {
        slack_send: { enabled: false, installation_id: "install-1", channel_ids: ["C1"] },
        mcp_server_ids: ["mcp-1"],
      },
    }, expect.any(Object));
  });

  it("removes Send to Slack config while preserving Memories and MCP settings", async () => {
    const user = userEvent.setup();
    renderSection({
      memories: { enabled: true },
      slack_send: { enabled: false },
      mcp_server_ids: ["mcp-1"],
    });

    await user.click(await screen.findByRole("button", { name: "Remove Send to Slack" }));
    expect(mocks.update).toHaveBeenCalledWith({
      id: "automation-1",
      tools: { memories: { enabled: true }, mcp_server_ids: ["mcp-1"] },
    }, expect.any(Object));
  });

  it("adds missing built-ins without dropping existing MCP settings", async () => {
    const user = userEvent.setup();
    renderSection({ mcp_server_ids: ["mcp-1"] });

    await user.click(await screen.findByRole("button", { name: "Add Tool or MCP" }));
    await user.click(screen.getByRole("button", { name: "Memories" }));
    expect(mocks.update).toHaveBeenLastCalledWith({
      id: "automation-1",
      tools: { memories: {}, mcp_server_ids: ["mcp-1"] },
    }, expect.any(Object));

    await user.click(screen.getByRole("button", { name: "Send to Slack" }));
    expect(mocks.update).toHaveBeenLastCalledWith({
      id: "automation-1",
      tools: { slack_send: {}, mcp_server_ids: ["mcp-1"] },
    }, expect.any(Object));
  });

  it("refreshes the channel catalog when the installed Slack set changes", async () => {
    const install1 = { id: "install-1", team_id: "T1", status: "installed" };
    const install2 = { id: "install-2", team_id: "T2", status: "installed" };
    mocks.slackList.mockResolvedValue({ installations: [install1] });
    mocks.slackCatalog.mockResolvedValue({
      channels: [{ installation_id: "install-1", team_id: "T1", id: "C1", name: "general" }],
    });
    const { queryClient } = renderSection({ slack_send: { enabled: false } });
    await screen.findByRole("button", { name: "Slack channels" });
    await waitFor(() => expect(mocks.slackCatalog).toHaveBeenCalled());
    mocks.slackCatalog.mockClear();

    act(() => {
      queryClient.setQueryData(["slack-installations"], { installations: [install1, install2] });
    });

    await waitFor(() => expect(mocks.slackCatalog).toHaveBeenCalled());
  });

  it("shows native MCP inline even with an empty workspace library and no settings jump", async () => {
    mocks.mcpList.mockResolvedValue([]);
    renderSection({}, { type: "agent", id: "agent-1" });
    expect(mocks.inventory).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole("button", { name: "Add Tool or MCP" }));
    expect(await screen.findByText("Native docs")).toBeInTheDocument();
    expect(screen.getByText("Inherited")).toBeInTheDocument();
    expect(screen.queryByText("No MCP servers in this workspace.")).not.toBeInTheDocument();
    expect(screen.queryByText("Additional MCP servers")).not.toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "Manage MCP servers" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Manage MCP servers" })).not.toBeInTheDocument();
    expect(mocks.update).not.toHaveBeenCalled();
  });

  it("keeps workspace selection functional without storing inherited server names", async () => {
    renderSection({ mcp_server_ids: [] }, { type: "agent", id: "agent-1" });
    await userEvent.click(screen.getByRole("button", { name: "Add Tool or MCP" }));
    await screen.findByText("Native docs");
    await userEvent.click(screen.getByRole("checkbox", { name: "Issue tracker" }));
    expect(mocks.update).toHaveBeenLastCalledWith({
      id: "automation-1", tools: { mcp_server_ids: ["mcp-1"] },
    }, expect.any(Object));
    expect(screen.queryByRole("checkbox", { name: "Native docs" })).not.toBeInTheDocument();
  });
});
