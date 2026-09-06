import { describe, it, expect, vi, beforeEach } from "vitest";
import { screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { renderWithI18n } from "../../test/i18n";

const mockUpdateAutomation = vi.hoisted(() => vi.fn());

vi.mock("@patchbay/core/hooks", () => ({ useWorkspaceId: () => "ws-test" }));

vi.mock("@patchbay/core/paths", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@patchbay/core/paths")>();
  return {
    ...actual,
    useWorkspacePaths: () => ({
      automations: () => "/acme/automations",
      automationDetail: (id: string) => `/acme/automations/${id}`,
      agentDetail: (id: string) => `/acme/agents/${id}`,
      projectDetail: (id: string) => `/acme/projects/${id}`,
      settings: () => "/acme/settings",
      issueDetail: (id: string) => `/acme/issues/${id}`,
    }),
    useRequiredWorkspaceSlug: () => "acme",
  };
});

vi.mock("@patchbay/core/workspace/hooks", () => ({
  useActorName: () => ({
    getActorName: (_type: string, id: string) => `Person ${id.slice(0, 6)}`,
  }),
}));

vi.mock("@patchbay/core/workspace/queries", () => ({
  agentListOptions: (wsId: string) => ({
    queryKey: ["agents", wsId],
    queryFn: async () => [
      { id: "agent-1", name: "Scout", archived_at: null, runtime_id: "runtime-1" },
    ],
  }),
  teamListOptions: (wsId: string) => ({
    queryKey: ["teams", wsId],
    queryFn: async () => [],
  }),
  workspaceMcpServersOptions: (wsId: string) => ({
    queryKey: ["mcp", wsId],
    queryFn: async () => [],
  }),
}));

vi.mock("@patchbay/core/projects/queries", () => ({
  projectDetailOptions: (wsId: string, id: string) => ({
    queryKey: ["project", wsId, id],
    queryFn: async () => null,
    enabled: Boolean(id),
  }),
  projectListOptions: (wsId: string) => ({
    queryKey: ["projects", wsId],
    queryFn: async () => [],
  }),
}));

vi.mock("@patchbay/core/github/queries", () => ({
  githubInstallationsOptions: (wsId: string) => ({
    queryKey: ["github", wsId],
    queryFn: async () => ({ installations: [] }),
  }),
}));

vi.mock("@patchbay/core/slack/queries", () => ({
  slackInstallationsOptions: (wsId: string) => ({
    queryKey: ["slack", wsId],
    queryFn: async () => ({ installations: [] }),
  }),
}));

vi.mock("@patchbay/core/linear/queries", () => ({
  linearConnectionOptions: (wsId: string) => ({
    queryKey: ["linear", wsId],
    queryFn: async () => ({ connected: false }),
  }),
}));

vi.mock("@patchbay/core/automations/queries", () => ({
  automationDetailOptions: () => ({
    queryKey: ["automation-detail"],
    queryFn: async () => ({
      automation: {
        id: "auto-1",
        workspace_id: "ws-test",
        title: "Find critical bugs",
        description: "## Goal\nFind bugs",
        project_id: null,
        executor_type: "agent",
        executor_id: "agent-1",
        status: "paused",
        execution_mode: "run_only",
        issue_title_template: null,
        model: "cursor-grok-4.6-high-fast",
        tools: {},
        created_by_type: "member",
        created_by_id: "user-1",
        last_run_at: null,
        created_at: "2026-01-01T00:00:00Z",
        updated_at: "2026-01-01T00:00:00Z",
        can_write: true,
        can_manage_access: true,
        subscribers: [],
      },
      triggers: [
        {
          id: "trg-1",
          automation_id: "auto-1",
          kind: "schedule",
          enabled: true,
          cron_expression: "0 7 * * *",
          timezone: "America/New_York",
          next_run_at: "2026-09-07T11:00:00Z",
          webhook_token: null,
          label: null,
          last_fired_at: null,
          created_at: "2026-01-01T00:00:00Z",
          updated_at: "2026-01-01T00:00:00Z",
        },
      ],
      collaborators: [],
    }),
  }),
  automationRunsOptions: () => ({
    queryKey: ["automation-runs"],
    queryFn: async () => [],
  }),
  automationRunOptions: () => ({
    queryKey: ["automation-run"],
    queryFn: async () => null,
  }),
}));

vi.mock("@patchbay/core/automations/mutations", () => ({
  useUpdateAutomation: () => ({ mutate: mockUpdateAutomation, mutateAsync: mockUpdateAutomation }),
  useDeleteAutomation: () => ({ mutateAsync: vi.fn() }),
  useTriggerAutomation: () => ({ mutateAsync: vi.fn(), isPending: false }),
  useCreateAutomationTrigger: () => ({ mutateAsync: vi.fn() }),
  useUpdateAutomationTrigger: () => ({ mutate: vi.fn(), mutateAsync: vi.fn() }),
  useDeleteAutomationTrigger: () => ({ mutateAsync: vi.fn() }),
  useRotateAutomationTriggerWebhookToken: () => ({ mutateAsync: vi.fn(), isPending: false }),
}));

vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn(), warning: vi.fn() } }));

vi.mock("../../navigation", () => ({
  useNavigation: () => ({ push: vi.fn() }),
  AppLink: ({ href, children }: { href: string; children: unknown }) => (
    <a href={href}>{children as never}</a>
  ),
}));

vi.mock("../../editor", () => ({
  ContentEditor: ({ value }: { value: string }) => (
    <div data-testid="instructions-editor">{value}</div>
  ),
  ReadonlyContent: ({ content }: { content: string }) => <div>{content}</div>,
}));

vi.mock("../../agents/components/model-dropdown", () => ({
  ModelDropdown: ({
    value,
    compact,
  }: {
    value: string;
    compact?: boolean;
  }) => (
    <button type="button" data-testid="model-dropdown" data-compact={compact === true ? "true" : "false"}>
      {value || "default-model"}
    </button>
  ),
}));

vi.mock("../../projects/components/project-picker", () => ({
  ProjectPicker: ({ triggerRender }: { triggerRender?: unknown }) => (
    <div data-testid="project-picker">{triggerRender as never}</div>
  ),
}));

vi.mock("./webhook-deliveries-section", () => ({
  WebhookDeliveriesSection: () => null,
}));

vi.mock("../../common/task-transcript", () => ({
  TranscriptButton: () => null,
}));

import { AutomationDetailPage } from "./automation-detail-page";

function renderPage() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return renderWithI18n(
    <QueryClientProvider client={qc}>
      <AutomationDetailPage automationId="auto-1" />
    </QueryClientProvider>,
  );
}

describe("AutomationDetailPage settings layout", () => {
  beforeEach(() => {
    mockUpdateAutomation.mockReset();
  });

  it("puts the title in the document body instead of a properties grid", async () => {
    renderPage();

    const title = await screen.findByTestId("automation-settings-title");
    expect(title).toHaveTextContent("Find critical bugs");
    expect(title).toHaveClass("text-display-sm");

    expect(screen.queryByText("Properties")).not.toBeInTheDocument();
    expect(screen.queryByText("Output Mode")).not.toBeInTheDocument();
    expect(screen.getByText("By Person user-1")).toBeInTheDocument();
    expect(screen.getByText("Inactive")).toBeInTheDocument();
    expect(screen.getByText("No project")).toBeInTheDocument();
  });

  it("uses underline Settings / Run History tabs", async () => {
    renderPage();

    expect(await screen.findByRole("tab", { name: "Settings" })).toHaveAttribute(
      "aria-selected",
      "true",
    );
    expect(screen.getByRole("tab", { name: "Run History" })).toBeInTheDocument();
  });

  it("keeps Add Trigger inside the Triggers card", async () => {
    renderPage();

    const card = await screen.findByTestId("automation-triggers-card");
    expect(card).toHaveTextContent("Add Trigger");
    expect(screen.getByRole("button", { name: /Add Trigger/ })).toBeInTheDocument();
  });

  it("embeds a compact model picker in Agent Instructions", async () => {
    renderPage();

    expect(await screen.findByText("Agent Instructions")).toBeInTheDocument();
    const editor = screen.getByTestId("instructions-editor");
    expect(editor).toHaveTextContent("## Goal");
    const model = screen.getByTestId("model-dropdown");
    expect(model).toHaveAttribute("data-compact", "true");
    expect(model).toHaveTextContent("cursor-grok-4.6-high-fast");
    expect(screen.getByTestId("automation-instructions")).toContainElement(model);
  });

  it("renders Cursor-style Tools rows", async () => {
    renderPage();

    const tools = await screen.findByTestId("automation-tools");
    expect(tools).toHaveTextContent("Memories");
    expect(tools).toHaveTextContent("Send to Slack");
    expect(tools).toHaveTextContent("Requires connection");
    expect(tools).toHaveTextContent("Add Tool or MCP");
    await waitFor(() => {
      expect(screen.getByText(/require additional authentication/)).toBeInTheDocument();
    });
  });
});
