import { describe, it, expect, vi, beforeEach } from "vitest";
import { screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { renderWithI18n } from "../../test/i18n";

const mocks = vi.hoisted(() => ({
  updateAutomation: vi.fn(),
  createTrigger: vi.fn(async () => ({ id: "trg-new" })),
  updateTrigger: vi.fn(),
  deleteTrigger: vi.fn(async () => {}),
  triggerNow: vi.fn(async () => ({ status: "running" })),
}));

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
        {
          id: "trg-2",
          automation_id: "auto-1",
          kind: "webhook",
          enabled: true,
          cron_expression: null,
          timezone: null,
          next_run_at: null,
          webhook_token: null,
          provider: "github",
          preset: "github.pull_request.opened",
          config: {},
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
  useUpdateAutomation: () => ({ mutate: mocks.updateAutomation, mutateAsync: mocks.updateAutomation }),
  useDeleteAutomation: () => ({ mutateAsync: vi.fn() }),
  useTriggerAutomation: () => ({ mutateAsync: mocks.triggerNow, isPending: false }),
  useCreateAutomationTrigger: () => ({ mutateAsync: mocks.createTrigger }),
  useUpdateAutomationTrigger: () => ({ mutate: mocks.updateTrigger, mutateAsync: mocks.updateTrigger }),
  useDeleteAutomationTrigger: () => ({ mutateAsync: mocks.deleteTrigger }),
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
    mocks.updateAutomation.mockReset();
    mocks.createTrigger.mockClear();
    mocks.updateTrigger.mockClear();
    mocks.deleteTrigger.mockClear();
    mocks.triggerNow.mockClear();
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
    expect(screen.getByText(/Every day at/)).toBeInTheDocument();
    expect(screen.getByRole("combobox", { name: "Time" })).toHaveTextContent("07:00");
    expect(screen.getByText(/Next run/)).toBeInTheDocument();
  });

  it("places Settings / Run History pills under the title", async () => {
    renderPage();

    const title = await screen.findByTestId("automation-settings-title");
    const settings = screen.getByRole("tab", { name: "Settings" });
    expect(settings).toHaveAttribute("aria-selected", "true");
    expect(screen.getByRole("tab", { name: "Run History" })).toBeInTheDocument();
    expect(title.compareDocumentPosition(settings) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(settings).toHaveClass("data-active:bg-muted");
    expect(settings).toHaveClass("after:hidden");
  });

  it("keeps Add Trigger inside the Triggers card", async () => {
    renderPage();

    const card = await screen.findByTestId("automation-triggers-card");
    expect(card).toHaveTextContent("Add Trigger");
    expect(screen.getByRole("button", { name: /Add Trigger/ })).toBeInTheDocument();
  });

  it("embeds a compact model picker in the instructions editor", async () => {
    renderPage();

    expect(await screen.findByTestId("automation-instructions")).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "Agent Instructions" })).not.toBeInTheDocument();
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
    expect(tools).toHaveTextContent("Add MCP");
    expect(screen.getByText(/triggers require additional authentication/)).toBeInTheDocument();
  });

  it("wires Connect, status, time, and GitHub presets to real writes", async () => {
    const user = userEvent.setup();
    renderPage();

    expect(await screen.findByTestId("automation-settings-title")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Run now" })).toBeDisabled();

    const connectHrefs = screen.getAllByRole("link", { name: /Connect/ }).map((node) => node.getAttribute("href"));
    expect(connectHrefs).toEqual(expect.arrayContaining([
      "/acme/settings?tab=github",
      "/acme/settings?tab=integrations",
    ]));

    await user.click(screen.getByRole("switch", { name: "Activate automation" }));
    expect(mocks.updateAutomation).toHaveBeenCalledWith({ id: "auto-1", status: "active" });

    await user.click(screen.getByRole("combobox", { name: "Time" }));
    await user.click(await screen.findByRole("option", { name: "08:00" }));
    expect(mocks.updateTrigger).toHaveBeenCalledWith(expect.objectContaining({
      automationId: "auto-1",
      triggerId: "trg-1",
      cron_expression: expect.stringMatching(/0 8 \* \* \*/),
    }));

    await user.click(screen.getByRole("button", { name: "Add Trigger" }));
    const github = await screen.findByRole("menuitem", { name: "GitHub" });
    github.focus();
    await user.keyboard("{ArrowRight}");
    await user.click(await screen.findByRole("menuitem", { name: "Draft opened" }));
    expect(mocks.createTrigger).toHaveBeenCalledWith({
      automationId: "auto-1",
      kind: "webhook",
      preset: "github.draft.opened",
    });

    await user.click(screen.getByRole("button", { name: "Manage" }));
    const memoriesSwitch = screen.getAllByRole("switch").at(-1);
    expect(memoriesSwitch).toBeTruthy();
    await user.click(memoriesSwitch!);
    expect(mocks.updateAutomation.mock.calls.some((call) => {
      const payload = call[0] as { id?: string; tools?: { memories?: { enabled?: boolean } } };
      return payload.id === "auto-1" && payload.tools?.memories?.enabled === true;
    })).toBe(true);
  });
});
