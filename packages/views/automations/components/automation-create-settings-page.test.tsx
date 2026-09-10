// @vitest-environment jsdom
import { forwardRef, useImperativeHandle, type ComponentProps } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { renderWithI18n } from "../../test/i18n";

const mocks = vi.hoisted(() => ({
  create: vi.fn(),
  trigger: vi.fn(),
  replace: vi.fn(),
  accepted: vi.fn(),
  error: vi.fn(),
  github: vi.fn(),
  template: "",
}));
vi.mock("@orvilo/core/auth", () => ({
  useAuthStore: (selector: (state: { user: { name: string } }) => unknown) =>
    selector({ user: { name: "Creator" } }),
}));
vi.mock("@orvilo/core/hooks", () => ({ useWorkspaceId: () => "ws-test" }));
vi.mock("@orvilo/core/paths", () => ({
  useCurrentWorkspace: () => ({ name: "Acme" }),
  useWorkspacePaths: () => ({
    automations: () => "/acme/automations",
    settings: () => "/acme/settings",
    automationDetail: (id: string) => `/acme/automations/${id}`,
  }),
}));
vi.mock("@orvilo/core/api", () => ({ api: {
  listGitHubInstallations: mocks.github,
  listSlackInstallations: vi.fn(),
  getLinearConnection: vi.fn(),
} }));
vi.mock("../../navigation", () => ({
  AppLink: ({ children, ...props }: ComponentProps<"a">) => (
    <a {...props}>{children}</a>
  ),
  useNavigation: () => ({
    replace: mocks.replace,
    searchParams: new URLSearchParams(
      mocks.template ? { template: mocks.template } : {},
    ),
  }),
}));
vi.mock("@orvilo/core/workspace/queries", () => ({
  agentListOptions: () => ({
    queryKey: ["agents"],
    queryFn: async () => [
      {
        id: "agent-1",
        name: "Scout",
        archived_at: null,
        runtime_id: "runtime-1",
      },
    ],
  }),
  teamListOptions: () => ({ queryKey: ["teams"], queryFn: async () => [] }),
}));
vi.mock("@orvilo/core/projects/queries", () => ({
  projectDetailOptions: () => ({
    queryKey: ["project"],
    queryFn: async () => null,
  }),
}));
vi.mock("@orvilo/core/automations/mutations", () => ({
  useCreateAutomation: () => ({ mutateAsync: mocks.create }),
  useCreateAutomationTrigger: () => ({ mutateAsync: mocks.trigger }),
}));
vi.mock("./schedule-editor/validate", () => ({
  useScheduleSubmitGate: () => ({ ensureAccepted: mocks.accepted }),
}));
vi.mock("sonner", () => ({ toast: { error: mocks.error, success: vi.fn() } }));
vi.mock("../../common/actor-avatar", () => ({ ActorAvatar: () => null }));
vi.mock("../../projects/components/project-picker", () => ({
  ProjectPicker: ({
    triggerRender,
    onUpdate,
  }: {
    triggerRender: React.ReactNode;
    onUpdate: (updates: { project_id: string }) => void;
  }) => (
    <div>
      {triggerRender}
      <button onClick={() => onUpdate({ project_id: "project-1" })}>
        Choose project
      </button>
    </div>
  ),
}));
vi.mock("../../editor", () => ({
  ContentEditor: forwardRef(
    function MockContentEditor(
      { value, onUpdate }: { value: string; onUpdate: (value: string) => void },
      ref,
    ) {
      useImperativeHandle(ref, () => ({ getMarkdown: () => value }));
      return (
        <textarea
          aria-label="Instructions editor"
          value={value}
          onChange={(event) => onUpdate(event.target.value)}
        />
      );
    },
  ),
  ReadonlyContent: ({ content }: { content: string }) => <div>{content}</div>,
}));
vi.mock("./automation-tools-section", () => ({
  AutomationToolsSection: ({
    onToolsChange,
  }: {
    onToolsChange: (tools: object) => void;
  }) => (
    <section>
      <h2>Tools</h2>
      <button onClick={() => onToolsChange({ memories: {} })}>
        Add memories
      </button>
    </section>
  ),
}));
vi.mock("./trigger-schedule-dialog", () => ({
  TriggerScheduleDialog: () => null,
}));
import { AutomationCreateSettingsPage } from "./automation-create-settings-page";
function renderPage() {
  return renderWithI18n(
    <QueryClientProvider
      client={
        new QueryClient({ defaultOptions: { queries: { retry: false } } })
      }
    >
      <AutomationCreateSettingsPage />
    </QueryClientProvider>,
  );
}
async function selectScout(user: ReturnType<typeof userEvent.setup>) {
  await user.click(
    screen.getByRole("button", { name: /Select agent or team/ }),
  );
  await user.click(await screen.findByRole("button", { name: /Scout/ }));
}
const create = () => screen.getByRole("button", { name: "New automation" });

describe("Settings-based automation creation", () => {
  beforeEach(() => {
    mocks.create.mockReset().mockResolvedValue({ id: "created-1" });
    mocks.trigger.mockReset().mockResolvedValue({});
    mocks.replace.mockReset();
    mocks.accepted.mockReset().mockResolvedValue(true);
    mocks.error.mockReset();
    mocks.github.mockReset().mockResolvedValue({ installations: [] });
    mocks.template = "";
  });
  it("opens directly on Settings with the shared sections and no old creation form", () => {
    renderPage();
    expect(screen.getByRole("tab", { name: "Settings" })).toHaveAttribute(
      "aria-selected",
      "true",
    );
    expect(screen.getByTestId("automation-instructions")).toBeInTheDocument();
    expect(screen.getByTestId("automation-triggers-card")).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "Tools" })).toBeInTheDocument();
    expect(screen.queryByText("Output mode")).not.toBeInTheDocument();
    expect(screen.queryByText("Runbook")).not.toBeInTheDocument();
    expect(mocks.create).not.toHaveBeenCalled();
  });
  it("reports missing required fields without creating an empty record", () => {
    renderPage();
    fireEvent.click(create());
    expect(
      screen.getByText("Enter a name for this automation."),
    ).toBeInTheDocument();
    expect(
      screen.getByText(
        "Choose the agent or team that will run this automation.",
      ),
    ).toBeInTheDocument();
    expect(mocks.create).not.toHaveBeenCalled();
  });
  it("shows a draft trigger connection beside its removal action and preserves draft values", async () => {
    mocks.template = "assign_pr_reviewers";
    renderPage();
    fireEvent.change(screen.getByRole("textbox", { name: "Name" }), { target: { value: "Keep my PR draft" } });
    fireEvent.change(screen.getByRole("textbox", { name: "Instructions editor" }), { target: { value: "Review carefully" } });
    const connect = await screen.findByRole("button", { name: "Connect" });
    expect(connect).toHaveAttribute("href", "/acme/settings?tab=github");
    expect(connect).toHaveAttribute("target", "_blank");
    fireEvent.click(connect);
    expect(screen.getByRole("textbox", { name: "Name" })).toHaveValue("Keep my PR draft");
    expect(screen.getByRole("textbox", { name: "Instructions editor" })).toHaveValue("Review carefully");
    expect(screen.getByRole("button", { name: "Remove trigger" })).toBeInTheDocument();
    expect(screen.getByText("Configure event conditions here after creation.")).toBeInTheDocument();
    expect(mocks.create).not.toHaveBeenCalled();
    expect(mocks.trigger).not.toHaveBeenCalled();
    expect(mocks.replace).not.toHaveBeenCalled();
  });
  it("saves the current Settings values and replaces the draft route with the real detail", async () => {
    const user = userEvent.setup();
    renderPage();
    await user.type(
      screen.getByRole("textbox", { name: "Name" }),
      "Nightly checks",
    );
    await selectScout(user);
    fireEvent.change(
      screen.getByRole("textbox", { name: "Instructions editor" }),
      { target: { value: "Latest instructions" } },
    );
    fireEvent.click(screen.getByRole("button", { name: "Add memories" }));
    fireEvent.click(screen.getByRole("button", { name: "Choose project" }));
    fireEvent.click(create());
    await waitFor(() =>
      expect(mocks.replace).toHaveBeenCalledWith("/acme/automations/created-1"),
    );
    expect(mocks.create).toHaveBeenCalledWith(
      expect.objectContaining({
        title: "Nightly checks",
        description: "Latest instructions",
        executor_type: "agent",
        executor_id: "agent-1",
        project_id: "project-1",
        tools: { memories: {} },
      }),
    );
  });
  it("keeps the Settings draft available after a failed create", async () => {
    mocks.create.mockRejectedValueOnce(new Error("Create unavailable"));
    const user = userEvent.setup();
    renderPage();
    await user.type(
      screen.getByRole("textbox", { name: "Name" }),
      "Keep my draft",
    );
    await selectScout(user);
    fireEvent.click(create());
    await waitFor(() =>
      expect(mocks.error).toHaveBeenCalledWith("Create unavailable"),
    );
    expect(screen.getByRole("textbox", { name: "Name" })).toHaveValue(
      "Keep my draft",
    );
    expect(mocks.replace).not.toHaveBeenCalled();
    expect(create()).not.toBeDisabled();
  });
  it("validates the template schedule before creating the automation", async () => {
    mocks.template = "find_critical_bugs";
    mocks.accepted.mockResolvedValue(false);
    const user = userEvent.setup();
    renderPage();
    await selectScout(user);
    fireEvent.click(create());
    await waitFor(() => expect(mocks.accepted).toHaveBeenCalled());
    expect(mocks.create).not.toHaveBeenCalled();
    mocks.accepted.mockResolvedValue(true);
    fireEvent.click(create());
    await waitFor(() =>
      expect(mocks.replace).toHaveBeenCalledWith("/acme/automations/created-1"),
    );
    expect(mocks.trigger).toHaveBeenCalledWith(
      expect.objectContaining({
        automationId: "created-1",
        kind: "schedule",
        cron_expression: expect.any(String),
        timezone: expect.any(String),
      }),
    );
  });
  it("keeps the created record reachable if a template trigger fails", async () => {
    mocks.template = "find_critical_bugs";
    mocks.trigger.mockRejectedValue(new Error("Trigger unavailable"));
    const user = userEvent.setup();
    renderPage();
    await selectScout(user);
    fireEvent.click(create());
    await waitFor(() =>
      expect(mocks.replace).toHaveBeenCalledWith("/acme/automations/created-1"),
    );
    expect(mocks.create).toHaveBeenCalledTimes(1);
    expect(mocks.error).toHaveBeenCalledWith(
      expect.stringContaining("Trigger unavailable"),
    );
  });
});
