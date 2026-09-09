import { describe, it, expect, vi, beforeEach } from "vitest";
import {
  act,
  fireEvent,
  renderHook,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { renderWithI18n } from "../../test/i18n";
import type { AutomationRun, AutomationTrigger } from "@orvilo/core/types";

const mocks = vi.hoisted(() => ({
  updateAutomation: vi.fn(),
  createTrigger: vi.fn(async () => ({ id: "trg-new" })),
  updateTrigger: vi.fn(),
  deleteTrigger: vi.fn(async () => {}),
  triggerNow: vi.fn(async () => ({ status: "running" })),
  automationRuns: [] as AutomationRun[],
  automationStatus: "paused" as "active" | "paused",
  automationRunReady: undefined as boolean | undefined,
  triggerEnabled: true,
  triggerReadinessReasons: [] as string[],
  githubInstallations: vi.fn(async () => ({
    installations: [] as Array<{
      id: string;
      status?: string;
      installation_status?: string;
    }>,
  })),
  slackInstallations: vi.fn(async () => ({
    installations: [] as Array<{
      id: string;
      status?: string;
      installation_status?: string;
    }>,
  })),
  linearConnection: vi.fn(async () => ({ connected: false })),
}));

vi.mock("@orvilo/core/hooks", () => ({ useWorkspaceId: () => "ws-test" }));
vi.mock("@orvilo/core/auth", () => ({
  useAuthStore: (selector: (state: { user: { id: string } }) => unknown) => selector({ user: { id: "user-1" } }),
}));

vi.mock("@orvilo/core/paths", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@orvilo/core/paths")>();
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

vi.mock("@orvilo/core/workspace/hooks", () => ({
  useActorName: () => ({
    getActorName: (_type: string, id: string) => `Person ${id.slice(0, 6)}`,
  }),
}));

vi.mock("@orvilo/core/workspace/queries", () => ({
  agentListOptions: (wsId: string) => ({
    queryKey: ["agents", wsId],
    queryFn: async () => [],
  }),
  workspaceMcpServersOptions: (wsId: string) => ({
    queryKey: ["mcp", wsId],
    queryFn: async () => [],
  }),
  teamListOptions: (wsId: string) => ({
    queryKey: ["teams", wsId],
    queryFn: async () => [],
  }),
}));

vi.mock("@orvilo/core/runtimes", async (importOriginal) => ({
  ...await importOriginal<typeof import("@orvilo/core/runtimes")>(),
  runtimeListOptions: (wsId: string) => ({
    queryKey: ["runtimes", wsId],
    queryFn: async () => [],
  }),
}));

vi.mock("@orvilo/core/projects/queries", () => ({
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

vi.mock("@orvilo/core/github/queries", () => ({
  githubInstallationsOptions: (wsId: string) => ({
    queryKey: ["github", wsId],
    queryFn: () => mocks.githubInstallations(),
  }),
  githubAutomationRepositoriesOptions: (
    wsId: string,
    _automationId: string,
    enabled: boolean,
  ) => ({
    queryKey: ["github-repositories", wsId],
    queryFn: async () => ({ repositories: [], me_logins: [] }),
    enabled,
  }),
}));

vi.mock("@orvilo/core/slack/queries", () => ({
  slackInstallationsOptions: (wsId: string) => ({
    queryKey: ["slack", wsId],
    queryFn: () => mocks.slackInstallations(),
  }),
  slackAutomationCatalogOptions: (wsId: string, enabled: boolean) => ({
    queryKey: ["slack-catalog", wsId],
    queryFn: async () => ({ channels: [] }),
    enabled,
  }),
}));

vi.mock("@orvilo/core/linear/queries", () => ({
  linearConnectionOptions: (wsId: string) => ({
    queryKey: ["linear", wsId],
    queryFn: () => mocks.linearConnection(),
  }),
  linearCatalogOptions: (wsId: string, enabled: boolean) => ({
    queryKey: ["linear-catalog", wsId],
    queryFn: async () => ({
      teams: [],
      projects: [],
      states: [],
      users: [],
      labels: [],
    }),
    enabled,
  }),
}));

vi.mock("@orvilo/core/automations/queries", () => ({
  cronPreviewOptions: (_ws: string, cron: string, timezone: string) => ({
    queryKey: ["cron-preview", cron, timezone],
    queryFn: async () => ({ runs: ["2026-09-07T11:00:00Z"] }),
  }),
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
        status: mocks.automationStatus,
        execution_mode: "run_only",
        issue_title_template: null,
        model: "cursor-grok-4.6-high-fast",
        tools: { memories: { enabled: false }, slack_send: { enabled: false } },
        created_by_type: "member",
        created_by_id: "user-1",
        last_run_at: null,
        created_at: "2026-01-01T00:00:00Z",
        updated_at: "2026-01-01T00:00:00Z",
        can_write: true,
        can_manage_access: true,
        subscribers: [],
        run_ready: mocks.automationRunReady,
      },
      triggers: [
        {
          id: "trg-1",
          automation_id: "auto-1",
          kind: "schedule",
          enabled: mocks.triggerEnabled,
          cron_expression: "0 7 * * *",
          timezone: "America/New_York",
          next_run_at: "2026-09-07T11:00:00Z",
          webhook_token: null,
          label: null,
          last_fired_at: null,
          created_at: "2026-01-01T00:00:00Z",
          updated_at: "2026-01-01T00:00:00Z",
          readiness_reasons: mocks.triggerReadinessReasons,
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
    queryFn: async () => mocks.automationRuns,
  }),
  automationRunOptions: () => ({
    queryKey: ["automation-run"],
    queryFn: async () => null,
  }),
}));

vi.mock("@orvilo/core/automations", async (importOriginal) => {
  const actual =
    await importOriginal<typeof import("@orvilo/core/automations")>();
  return {
    ...actual,
    automationMemoryKeys: {
      list: () => ["automation-memory-list"],
      detail: (_wsId: string, _automationId: string, name: string) => [
        "automation-memory",
        name,
      ],
    },
    automationMemoryListOptions: () => ({
      queryKey: ["automation-memory-list"],
      queryFn: async () => ({ items: [] }),
    }),
    automationMemoryOptions: (
      _wsId: string,
      _automationId: string,
      name: string,
      options?: { enabled?: boolean },
    ) => ({
      queryKey: ["automation-memory", name],
      queryFn: async () => ({
        name,
        content: "",
        revision: 1,
        updated_at: "now",
      }),
      enabled: options?.enabled ?? true,
    }),
    useUpdateAutomationMemory: () => ({
      mutateAsync: vi.fn(),
      isPending: false,
    }),
    useDeleteAutomationMemory: () => ({
      mutateAsync: vi.fn(),
      isPending: false,
    }),
  };
});

vi.mock("@orvilo/core/automations/mutations", () => ({
  useUpdateAutomation: () => ({
    mutate: mocks.updateAutomation,
    mutateAsync: mocks.updateAutomation,
  }),
  useDeleteAutomation: () => ({ mutateAsync: vi.fn() }),
  useTriggerAutomation: () => ({
    mutateAsync: mocks.triggerNow,
    isPending: false,
  }),
  useCreateAutomationTrigger: () => ({ mutateAsync: mocks.createTrigger }),
  useUpdateAutomationTrigger: () => ({
    mutate: mocks.updateTrigger,
    mutateAsync: mocks.updateTrigger,
  }),
  useDeleteAutomationTrigger: () => ({ mutateAsync: mocks.deleteTrigger }),
  useRotateAutomationTriggerWebhookToken: () => ({
    mutateAsync: vi.fn(),
    isPending: false,
  }),
}));

vi.mock("sonner", () => ({
  toast: { success: vi.fn(), error: vi.fn(), warning: vi.fn() },
}));

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

vi.mock("./pickers/agent-picker", () => ({
  AgentPicker: ({
    assignee,
    disabled,
    onChange,
  }: {
    assignee: { type: string; id: string };
    disabled?: boolean;
    onChange: (next: { type: "agent" | "team"; id: string }) => void;
  }) => (
    <div
      data-testid="agent-picker"
      data-assignee-type={assignee.type}
      data-assignee-id={assignee.id}
    >
      <button
        type="button"
        disabled={disabled}
        onClick={() => onChange({ type: "agent", id: "agent-2" })}
      >
        Select agent
      </button>
    </div>
  ),
}));

vi.mock("../../projects/components/project-picker", () => ({
  ProjectPicker: ({ triggerRender }: { triggerRender?: unknown }) => (
    <div data-testid="project-picker">{triggerRender as never}</div>
  ),
}));

vi.mock("./webhook-deliveries-section", () => ({
  WebhookDeliveriesSection: ({
    hasWebhookTrigger,
  }: {
    hasWebhookTrigger: boolean;
  }) => (hasWebhookTrigger ? <div data-testid="event-deliveries" /> : null),
}));

vi.mock("../../agent-thread", () => ({
  AgentThreadButton: ({
    task,
    title,
  }: {
    task: { id: string; workspace_id?: string };
    title: string;
  }) => (
    <button
      type="button"
      data-testid="agent-thread-entrypoint"
      data-task-id={task.id}
      data-workspace-id={task.workspace_id}
    >
      {title}
    </button>
  ),
}));

import {
  AutomationDetailPage,
  useRunHistoryClock,
} from "./automation-detail-page";
import { TriggerCard } from "./trigger-card";

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
    mocks.updateTrigger.mockReset().mockResolvedValue(undefined);
    mocks.deleteTrigger.mockClear();
    mocks.triggerNow.mockClear();
    mocks.automationStatus = "paused";
    mocks.automationRunReady = undefined;
    mocks.triggerEnabled = true;
    mocks.triggerReadinessReasons = [];
    mocks.automationRuns.splice(0);
    mocks.githubInstallations
      .mockReset()
      .mockResolvedValue({ installations: [] });
    mocks.slackInstallations
      .mockReset()
      .mockResolvedValue({ installations: [] });
    mocks.linearConnection.mockReset().mockResolvedValue({ connected: false });
  });

  it("opens every task-backed run in the Agent conversation", async () => {
    mocks.automationRuns.push(
      ...(["running", "completed", "failed"] as const).map((status) => ({
        id: `run-${status}`,
        automation_id: "auto-1",
        trigger_id: null,
        source: "manual" as const,
        status,
        issue_id: null,
        task_id: `task-${status}`,
        triggered_at: "2026-09-06T12:00:00Z",
        completed_at: status === "running" ? null : "2026-09-06T12:01:00Z",
        failure_reason: status === "failed" ? "startup failed" : null,
        trigger_payload: null,
        result: null,
        created_at: "2026-09-06T12:00:00Z",
      })),
      {
        id: "run-issue-linked",
        automation_id: "auto-1",
        trigger_id: null,
        source: "manual",
        status: "completed",
        issue_id: "issue-1",
        task_id: "task-issue-linked",
        triggered_at: "2026-09-06T12:00:00Z",
        completed_at: "2026-09-06T12:01:00Z",
        failure_reason: null,
        trigger_payload: null,
        result: null,
        created_at: "2026-09-06T12:00:00Z",
      },
    );
    const user = userEvent.setup();
    renderPage();

    await user.click(await screen.findByRole("tab", { name: "Run History" }));

    for (const heading of [
      "Trigger",
      "Triggered",
      "Tools",
      "Status",
      "Duration",
    ]) {
      expect(
        screen.getByRole("columnheader", { name: heading }),
      ).toBeInTheDocument();
    }
    expect(screen.getAllByText("Test run")).toHaveLength(4);
    expect(
      screen.queryByText("cursor-grok-4.6-high-fast"),
    ).not.toBeInTheDocument();
    expect(screen.getAllByText("1m")).toHaveLength(3);

    const entrypoints = await screen.findAllByTestId("agent-thread-entrypoint");
    expect(entrypoints).toHaveLength(4);
    expect(
      entrypoints.map((button) => button.getAttribute("data-task-id")),
    ).toEqual([
      "task-running",
      "task-completed",
      "task-failed",
      "task-issue-linked",
    ]);
    for (const button of entrypoints) {
      expect(button).toHaveAttribute("data-workspace-id", "ws-test");
      expect(button).toHaveTextContent("View conversation");
    }
  });

  it("searches and filters the run-history table", async () => {
    mocks.automationRuns.push(
      {
        id: "run-completed",
        automation_id: "auto-1",
        trigger_id: null,
        source: "manual",
        status: "completed",
        issue_id: null,
        task_id: null,
        triggered_at: "2026-09-06T12:00:00Z",
        completed_at: "2026-09-06T12:01:00Z",
        failure_reason: null,
        trigger_payload: null,
        result: null,
        created_at: "2026-09-06T12:00:00Z",
      },
      {
        id: "run-failed",
        automation_id: "auto-1",
        trigger_id: null,
        source: "webhook",
        status: "failed",
        issue_id: null,
        task_id: null,
        triggered_at: "2026-09-06T13:00:00Z",
        completed_at: "2026-09-06T13:00:20Z",
        failure_reason: "Provider rejected payload",
        trigger_payload: null,
        result: null,
        created_at: "2026-09-06T13:00:00Z",
      },
    );
    const user = userEvent.setup();
    renderPage();

    await user.click(await screen.findByRole("tab", { name: "Run History" }));
    await user.click(
      screen.getByRole("button", { name: "Search run history" }),
    );
    const search = screen.getByRole("textbox", { name: "Search runs…" });
    await user.type(search, "Provider rejected");
    expect(screen.getAllByTestId("automation-run-row")).toHaveLength(1);
    expect(screen.getByText("Failed")).toBeInTheDocument();
    expect(screen.getByText("Provider rejected payload")).toBeVisible();

    await user.clear(search);
    await user.click(
      screen.getByRole("button", { name: "Filter run history" }),
    );
    await user.click(
      await screen.findByRole("menuitemcheckbox", { name: "Completed" }),
    );
    const [completedRow] = screen.getAllByTestId("automation-run-row");
    expect(screen.getAllByTestId("automation-run-row")).toHaveLength(1);
    expect(within(completedRow!).getByText("Completed")).toBeInTheDocument();
  });

  it("refreshes the run-history clock while a run remains active", () => {
    const initialNow = Date.parse("2026-09-06T12:02:01Z");
    vi.useFakeTimers();
    vi.setSystemTime(initialNow);
    try {
      const { result } = renderHook(() => useRunHistoryClock(true));
      expect(result.current).toBe(initialNow);

      act(() => {
        vi.advanceTimersByTime(60_000);
      });
      expect(result.current).toBe(initialNow + 60_000);
    } finally {
      vi.useRealTimers();
    }
  });

  it("refreshes immediately when the first active run appears", () => {
    const initialNow = Date.parse("2026-09-06T12:02:01Z");
    vi.useFakeTimers();
    vi.setSystemTime(initialNow);
    try {
      const { result, rerender } = renderHook(
        ({ active }) => useRunHistoryClock(active),
        { initialProps: { active: false } },
      );
      expect(result.current).toBe(initialNow);

      vi.setSystemTime(initialNow + 5 * 60_000);
      rerender({ active: true });
      expect(result.current).toBe(initialNow + 5 * 60_000);
    } finally {
      vi.useRealTimers();
    }
  });

  it("shows completion tool delivery errors on the run row", async () => {
    mocks.automationRuns.push({
      id: "run-delivery-failed",
      automation_id: "auto-1",
      trigger_id: null,
      source: "schedule",
      status: "completed",
      issue_id: null,
      task_id: "task-delivery-failed",
      triggered_at: "2026-09-06T12:00:00Z",
      completed_at: "2026-09-06T12:01:00Z",
      failure_reason: null,
      trigger_payload: null,
      result: {
        tool_delivery_errors: [
          { tool: "slack", message: "Slack could not post to #alerts" },
        ],
      },
      created_at: "2026-09-06T12:00:00Z",
    });
    const user = userEvent.setup();
    renderPage();

    await user.click(await screen.findByRole("tab", { name: "Run History" }));

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Completion delivery failed: Slack could not post to #alerts",
    );
  });

  it("opens the complete schedule editor and updates the existing schedule", async () => {
    const user = userEvent.setup();
    renderPage();
    await user.click(
      await screen.findByRole("button", { name: "Edit schedule" }),
    );
    const dialog = screen.getByRole("dialog", { name: "Edit schedule" });
    await user.click(
      within(dialog).getByRole("combobox", { name: "Day pattern" }),
    );
    await user.click(screen.getByRole("option", { name: "Day of month" }));
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    expect(mocks.updateTrigger).toHaveBeenCalledWith(
      expect.objectContaining({
        automationId: "auto-1",
        triggerId: "trg-1",
        cron_expression: expect.stringMatching(/0 7 \d+ \* \*/),
        timezone: "America/New_York",
      }),
    );
    expect(mocks.createTrigger).not.toHaveBeenCalled();
  });

  it("keeps MCP availability inline when the workspace library is empty", async () => {
    const user = userEvent.setup();
    renderPage();
    await user.click(
      await screen.findByRole("button", { name: "Add Tool or MCP" }),
    );
    expect(await screen.findByTestId("automation-inherited-mcp")).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "Manage MCP servers" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Manage MCP servers" })).not.toBeInTheDocument();
  });

  it("opens the real memory notes dialog from Manage", async () => {
    const user = userEvent.setup();
    renderPage();
    await user.click(await screen.findByRole("button", { name: "Manage" }));
    expect(
      await screen.findByRole("dialog", { name: "Memory Notes" }),
    ).toBeInTheDocument();
  });

  it("shows native event deliveries even without a generic webhook URL", async () => {
    renderPage();
    expect(
      await screen.findByTestId("automation-settings-title"),
    ).toBeInTheDocument();
    expect(screen.getByTestId("event-deliveries")).toBeInTheDocument();
  });

  it("does not offer keyword filters that Slack reaction payloads cannot supply", async () => {
    const user = userEvent.setup();
    mocks.slackInstallations.mockResolvedValue({
      installations: [
        {
          id: "slack-install-1",
          status: "installed",
          installation_status: "installed",
        },
      ],
    });
    const qc = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });
    const trigger = {
      id: "reaction-1",
      automation_id: "auto-1",
      kind: "webhook",
      enabled: true,
      provider: "slack",
      preset: "slack.reaction",
      config: {},
      cron_expression: null,
      timezone: null,
      next_run_at: null,
      webhook_token: null,
      label: null,
      last_fired_at: null,
      created_at: "",
      updated_at: "",
    } satisfies AutomationTrigger;
    renderWithI18n(
      <QueryClientProvider client={qc}>
        <TriggerCard trigger={trigger} automationId="auto-1" canWrite />
      </QueryClientProvider>,
    );
    await user.click(await screen.findByRole("button", { name: "Any Emoji" }));
    expect(
      screen.getByRole("combobox", { name: "Channel ID" }),
    ).toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: "Emoji" })).toBeInTheDocument();
    expect(
      screen.queryByRole("textbox", { name: "Keyword" }),
    ).not.toBeInTheDocument();
  });

  it("hides native trigger conditions until its provider is connected", async () => {
    const qc = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });
    const trigger = {
      id: "github-1",
      automation_id: "auto-1",
      kind: "webhook",
      enabled: true,
      provider: "github",
      preset: "github.push_to_branch",
      config: {
        repositories: ["acme/app"],
        branch: "main",
        author_scope: "specific",
        author_logins: ["octocat"],
      },
      cron_expression: null,
      timezone: null,
      next_run_at: null,
      webhook_token: null,
      label: null,
      last_fired_at: null,
      created_at: "",
      updated_at: "",
    } satisfies AutomationTrigger;
    renderWithI18n(
      <QueryClientProvider client={qc}>
        <TriggerCard trigger={trigger} automationId="auto-1" canWrite />
      </QueryClientProvider>,
    );

    expect(await screen.findByText("Requires connection")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Connect" })).toHaveAttribute(
      "href",
      "/acme/settings?tab=github",
    );
    expect(screen.queryByRole("combobox")).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "Filters" }),
    ).not.toBeInTheDocument();
    expect(screen.queryByText("acme/app")).not.toBeInTheDocument();
  });

  it("keeps an unready active run clickable and explains setup without raw readiness details", async () => {
    const user = userEvent.setup();
    mocks.automationStatus = "active";
    mocks.automationRunReady = false;
    mocks.triggerReadinessReasons = [
      "Trigger 123: Schedule has no valid next run.",
    ];
    renderPage();

    const runNow = await screen.findByRole("button", { name: "Run now" });
    expect(runNow).toBeEnabled();
    await user.click(runNow);

    expect(mocks.triggerNow).not.toHaveBeenCalled();
    const dialog = await screen.findByRole("alertdialog", {
      name: "Finish trigger setup before running",
    });
    expect(dialog).toHaveTextContent(
      "Connect the required services and finish configuring the triggers, then try again.",
    );
    expect(dialog).toHaveTextContent("Back to configuration");
    expect(
      screen.queryByText("Trigger 123: Schedule has no valid next run."),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByText(
        "Configure the triggers before running this automation",
      ),
    ).not.toBeInTheDocument();

    await user.click(
      within(dialog).getByRole("button", { name: "Back to configuration" }),
    );
    expect(
      screen.queryByRole("alertdialog", {
        name: "Finish trigger setup before running",
      }),
    ).not.toBeInTheDocument();
  });

  it("localizes a disabled legacy schedule without showing its raw readiness reason", async () => {
    const qc = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });
    const trigger = {
      id: "legacy-schedule-1",
      automation_id: "auto-1",
      kind: "schedule",
      enabled: false,
      cron_expression: "0 7 * * *",
      timezone: "America/New_York",
      next_run_at: null,
      webhook_token: null,
      label: null,
      last_fired_at: null,
      created_at: "",
      updated_at: "",
      readiness_reasons: [
        "This legacy trigger is disabled; delete and add it again to use it.",
      ],
    } satisfies AutomationTrigger;
    renderWithI18n(
      <QueryClientProvider client={qc}>
        <TriggerCard trigger={trigger} automationId="auto-1" canWrite />
      </QueryClientProvider>,
    );

    expect(
      await screen.findByText("Needs reconfiguration"),
    ).toBeInTheDocument();
    expect(
      screen.queryByText(/This legacy trigger is disabled/),
    ).not.toBeInTheDocument();
  });

  it("keeps native conditions hidden while connection status is loading", async () => {
    mocks.githubInstallations.mockImplementation(() => new Promise(() => {}));
    const qc = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });
    const trigger = {
      id: "github-loading-1",
      automation_id: "auto-1",
      kind: "webhook",
      enabled: true,
      provider: "github",
      preset: "github.pull_request.review_submitted",
      config: {},
      cron_expression: null,
      timezone: null,
      next_run_at: null,
      webhook_token: null,
      label: null,
      last_fired_at: null,
      created_at: "",
      updated_at: "",
    } satisfies AutomationTrigger;
    renderWithI18n(
      <QueryClientProvider client={qc}>
        <TriggerCard trigger={trigger} automationId="auto-1" canWrite />
      </QueryClientProvider>,
    );

    expect(
      await screen.findByText("Checking connection..."),
    ).toBeInTheDocument();
    expect(screen.queryByRole("combobox")).not.toBeInTheDocument();
    expect(
      screen.queryByRole("link", { name: "Connect" }),
    ).not.toBeInTheDocument();
  });

  it("keeps native conditions hidden and offers retry when connection lookup fails", async () => {
    mocks.githubInstallations.mockRejectedValue(
      new Error("provider unavailable"),
    );
    const qc = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });
    const trigger = {
      id: "github-failed-1",
      automation_id: "auto-1",
      kind: "webhook",
      enabled: true,
      provider: "github",
      preset: "github.pull_request.review_submitted",
      config: {},
      cron_expression: null,
      timezone: null,
      next_run_at: null,
      webhook_token: null,
      label: null,
      last_fired_at: null,
      created_at: "",
      updated_at: "",
    } satisfies AutomationTrigger;
    renderWithI18n(
      <QueryClientProvider client={qc}>
        <TriggerCard trigger={trigger} automationId="auto-1" canWrite />
      </QueryClientProvider>,
    );

    expect(
      await screen.findByText("Could not check connection"),
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Retry" })).toBeInTheDocument();
    expect(screen.queryByRole("combobox")).not.toBeInTheDocument();
    expect(
      screen.queryByRole("link", { name: "Connect" }),
    ).not.toBeInTheDocument();
  });

  it("keeps the latest filter edit queued until the previous save finishes", async () => {
    let finishFirst!: () => void;
    const firstSave = new Promise<void>((resolve) => {
      finishFirst = resolve;
    });
    mocks.updateTrigger
      .mockReturnValueOnce(firstSave)
      .mockResolvedValue(undefined);
    const user = userEvent.setup();
    mocks.githubInstallations.mockResolvedValue({
      installations: [{ id: "github-install-1" }],
    });
    const qc = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });
    const trigger = {
      id: "label-1",
      automation_id: "auto-1",
      kind: "webhook",
      enabled: true,
      provider: "github",
      preset: "github.pull_request.label_changed",
      config: {},
      cron_expression: null,
      timezone: null,
      next_run_at: null,
      webhook_token: null,
      label: null,
      last_fired_at: null,
      created_at: "",
      updated_at: "",
    } satisfies AutomationTrigger;
    renderWithI18n(
      <QueryClientProvider client={qc}>
        <TriggerCard trigger={trigger} automationId="auto-1" canWrite />
      </QueryClientProvider>,
    );
    await user.click(await screen.findByRole("button", { name: "Filters" }));
    const label = screen.getByRole("textbox", { name: "Label" });
    fireEvent.change(label, { target: { value: "bug" } });
    fireEvent.blur(label);
    expect(mocks.updateTrigger).toHaveBeenCalledTimes(1);
    fireEvent.change(label, { target: { value: "security" } });
    fireEvent.blur(label);
    try {
      expect(mocks.updateTrigger).toHaveBeenCalledTimes(1);
    } finally {
      await act(async () => finishFirst());
    }
    await waitFor(() => expect(mocks.updateTrigger).toHaveBeenCalledTimes(2));
    expect(mocks.updateTrigger).toHaveBeenLastCalledWith(
      expect.objectContaining({
        config: { label: "security" },
      }),
    );
  });

  it("saves a specific GitHub review result instead of accepting every review", async () => {
    const user = userEvent.setup();
    mocks.githubInstallations.mockResolvedValue({
      installations: [{ id: "github-install-1" }],
    });
    const qc = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });
    const trigger = {
      id: "review-1",
      automation_id: "auto-1",
      kind: "webhook",
      enabled: true,
      provider: "github",
      preset: "github.pull_request.review_submitted",
      config: {},
      cron_expression: null,
      timezone: null,
      next_run_at: null,
      webhook_token: null,
      label: null,
      last_fired_at: null,
      created_at: "",
      updated_at: "",
    } satisfies AutomationTrigger;
    renderWithI18n(
      <QueryClientProvider client={qc}>
        <TriggerCard trigger={trigger} automationId="auto-1" canWrite />
      </QueryClientProvider>,
    );
    await user.click(await screen.findByRole("button", { name: "Filters" }));
    await user.click(
      await screen.findByRole("combobox", { name: "Review result" }),
    );
    await user.click(
      await screen.findByRole("option", { name: "Changes requested" }),
    );
    await waitFor(() =>
      expect(mocks.updateTrigger).toHaveBeenCalledWith(
        expect.objectContaining({
          config: { review_state: "changes_requested" },
        }),
      ),
    );
  });

  it("does not add branch or label controls to PR opened", () => {
    const qc = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });
    const trigger = {
      id: "pr-opened-1",
      automation_id: "auto-1",
      kind: "webhook",
      enabled: true,
      provider: "github",
      preset: "github.pull_request.opened",
      config: {},
      cron_expression: null,
      timezone: null,
      next_run_at: null,
      webhook_token: null,
      label: null,
      last_fired_at: null,
      created_at: "",
      updated_at: "",
    } satisfies AutomationTrigger;
    renderWithI18n(
      <QueryClientProvider client={qc}>
        <TriggerCard trigger={trigger} automationId="auto-1" canWrite />
      </QueryClientProvider>,
    );
    expect(
      screen.queryByRole("button", { name: "Filters" }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("textbox", { name: "Branch" }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("textbox", { name: "Label" }),
    ).not.toBeInTheDocument();
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
    expect(screen.getByRole("combobox", { name: "Time" })).toHaveTextContent(
      "07:00",
    );
    expect(screen.getByText(/Next run/)).toBeInTheDocument();
  });

  it("places Settings / Run History pills under the title", async () => {
    renderPage();

    const title = await screen.findByTestId("automation-settings-title");
    const settings = screen.getByRole("tab", { name: "Settings" });
    expect(settings).toHaveAttribute("aria-selected", "true");
    expect(
      screen.getByRole("tab", { name: "Run History" }),
    ).toBeInTheDocument();
    expect(
      title.compareDocumentPosition(settings) &
        Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
    expect(settings).toHaveClass("data-active:bg-muted");
    expect(settings).toHaveClass("after:hidden");
  });

  it("keeps Add Trigger inside the Triggers card", async () => {
    renderPage();

    const card = await screen.findByTestId("automation-triggers-card");
    expect(card).toHaveTextContent("Add Trigger");
    expect(
      screen.getByRole("button", { name: /Add Trigger/ }),
    ).toBeInTheDocument();
  });

  it("embeds the current Agent picker and clears legacy model overrides on change", async () => {
    renderPage();

    expect(
      await screen.findByTestId("automation-instructions"),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("heading", { name: "Instructions", level: 2 }),
    ).toBeInTheDocument();
    const editor = screen.getByTestId("instructions-editor");
    expect(editor).toHaveTextContent("## Goal");
    const picker = screen.getByTestId("agent-picker");
    expect(picker).toHaveAttribute("data-assignee-type", "agent");
    expect(picker).toHaveAttribute("data-assignee-id", "agent-1");
    expect(screen.getByTestId("automation-instructions")).toContainElement(
      picker,
    );

    await userEvent
      .setup()
      .click(screen.getByRole("button", { name: "Select agent" }));
    expect(mocks.updateAutomation).toHaveBeenCalledWith(
      {
        id: "auto-1",
        executor_type: "agent",
        executor_id: "agent-2",
        model: "",
      },
      expect.any(Object),
    );
  });

  it("renders Cursor-style Tools rows", async () => {
    renderPage();

    const tools = await screen.findByTestId("automation-tools");
    expect(tools).toHaveTextContent("Memories");
    expect(tools).toHaveTextContent("Send to Slack");
    await waitFor(() => expect(tools).toHaveTextContent("Requires connection"));
    expect(tools).toHaveTextContent("Add Tool or MCP");
  });

  it("wires Connect, status, time, and GitHub presets to real writes", async () => {
    const user = userEvent.setup();
    renderPage();

    expect(
      await screen.findByTestId("automation-settings-title"),
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Run now" })).toBeDisabled();

    await waitFor(() => {
      const connectHrefs = screen
        .getAllByRole("link", { name: /Connect/ })
        .map((node) => node.getAttribute("href"));
      expect(connectHrefs).toEqual(
        expect.arrayContaining([
          "/acme/settings?tab=github",
          "/acme/settings?tab=integrations",
        ]),
      );
    });

    await user.click(
      screen.getByRole("switch", { name: "Activate automation" }),
    );
    expect(mocks.updateAutomation).toHaveBeenCalledWith(
      { id: "auto-1", status: "active" },
      expect.any(Object),
    );

    await user.click(screen.getByRole("combobox", { name: "Time" }));
    await user.click(await screen.findByRole("option", { name: "08:00" }));
    expect(mocks.updateTrigger).toHaveBeenCalledWith(
      expect.objectContaining({
        automationId: "auto-1",
        triggerId: "trg-1",
        cron_expression: expect.stringMatching(/0 8 \* \* \*/),
      }),
      expect.any(Object),
    );

    await user.click(screen.getByRole("button", { name: "Add Trigger" }));
    const github = await screen.findByRole("menuitem", { name: "GitHub" });
    github.focus();
    await user.keyboard("{ArrowRight}");
    await user.click(
      await screen.findByRole("menuitem", { name: "Draft opened" }),
    );
    expect(mocks.createTrigger).toHaveBeenCalledWith({
      automationId: "auto-1",
      kind: "webhook",
      preset: "github.draft.opened",
    });

    expect(
      screen.queryByRole("switch", { name: "Memories" }),
    ).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Manage" })).toBeInTheDocument();
  });
});
