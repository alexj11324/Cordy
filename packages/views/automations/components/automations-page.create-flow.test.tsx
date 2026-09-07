// @vitest-environment jsdom
import type { ComponentProps } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Automation } from "@orvilo/core/types";
import { renderWithI18n } from "../../test/i18n";

const mocks = vi.hoisted(() => ({
  automations: [] as Automation[],
  listError: null as Error | null,
  push: vi.fn(),
}));
vi.mock("@orvilo/core/auth", () => ({
  useAuthStore: (selector: (state: { user: { id: string } }) => unknown) =>
    selector({ user: { id: "user-1" } }),
}));
vi.mock("@orvilo/core/hooks", () => ({ useWorkspaceId: () => "ws-test" }));
vi.mock("@orvilo/core/paths", () => ({
  useWorkspacePaths: () => ({
    newAutomation: (template?: string) =>
      `/acme/automations/new${template ? `?template=${template}` : ""}`,
    automationRuns: () => "/acme/automations/runs",
    automationDetail: (id: string) => `/acme/automations/${id}`,
  }),
}));
vi.mock("../../navigation", () => ({
  AppLink: ({ href, children, ...props }: ComponentProps<"a">) => (
    <a href={href} {...props}>
      {children}
    </a>
  ),
  useNavigation: () => ({ push: mocks.push }),
  useRowLink: () => () => ({}),
}));
vi.mock("@orvilo/core/workspace/hooks", () => ({
  useActorName: () => ({ getActorName: () => "Scout" }),
}));
vi.mock("@orvilo/core/automations/queries", () => ({
  automationListOptions: () => ({
    queryKey: ["automations", "ws-test"],
    queryFn: async () => {
      if (mocks.listError) throw mocks.listError;
      return mocks.automations;
    },
  }),
}));
vi.mock("../../common/actor-avatar", () => ({
  ActorAvatar: () => <span data-testid="actor-avatar" />,
}));
vi.mock("./automation-list-actions", () => ({
  AutomationBatchToolbar: () => <div data-testid="automation-batch-toolbar" />,
  AutomationRowActions: () => null,
}));
vi.mock("./automation-template-gallery", () => ({
  AutomationTemplateGallery: ({
    onSelectTemplate,
  }: {
    onSelectTemplate: (template: { id: string }) => void;
  }) => (
    <div data-testid="template-gallery">
      <button onClick={() => onSelectTemplate({ id: "find_critical_bugs" })}>
        Choose recommended automation
      </button>
    </div>
  ),
}));
import { AutomationsPage } from "./automations-page";

const EXISTING: Automation = {
  id: "automation-1",
  workspace_id: "ws-test",
  title: "Existing automation",
  description: null,
  project_id: null,
  executor_type: "agent",
  executor_id: "agent-1",
  status: "active",
  execution_mode: "run_only",
  issue_title_template: null,
  created_by_type: "member",
  created_by_id: "user-1",
  last_run_at: null,
  created_at: "2026-09-07T12:00:00Z",
  updated_at: "2026-09-07T12:00:00Z",
};
function renderPage() {
  return renderWithI18n(
    <QueryClientProvider
      client={
        new QueryClient({ defaultOptions: { queries: { retry: false } } })
      }
    >
      <AutomationsPage />
    </QueryClientProvider>,
  );
}

describe("AutomationsPage creation navigation", () => {
  beforeEach(() => {
    mocks.automations = [EXISTING];
    mocks.listError = null;
    mocks.push.mockReset();
  });
  it("keeps existing automations above the template grid", async () => {
    renderPage();
    const gallery = await screen.findByTestId("template-gallery");
    expect(
      screen
        .getByRole("heading", { name: "Your automations" })
        .compareDocumentPosition(gallery) & Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
    expect(screen.queryByRole("dialog")).toBeNull();
  });
  it("opens the Settings creation route from the top action", async () => {
    renderPage();
    await screen.findByTestId("template-gallery");
    fireEvent.click(screen.getByRole("button", { name: "New automation" }));
    expect(mocks.push).toHaveBeenCalledWith("/acme/automations/new");
    expect(screen.queryByRole("dialog")).toBeNull();
  });
  it("opens that same Settings route with the selected template", async () => {
    renderPage();
    fireEvent.click(
      await screen.findByRole("button", {
        name: "Choose recommended automation",
      }),
    );
    expect(mocks.push).toHaveBeenCalledWith(
      "/acme/automations/new?template=find_critical_bugs",
    );
  });
  it("keeps the new Settings route reachable if the list fails", async () => {
    mocks.listError = new Error("Automation list unavailable");
    renderPage();
    await screen.findByRole("alert");
    fireEvent.click(screen.getByRole("button", { name: "New automation" }));
    expect(mocks.push).toHaveBeenCalledWith("/acme/automations/new");
  });
  it("filters Mine by creator and exposes a link to All Runs", async () => {
    mocks.automations = [
      EXISTING,
      {
        ...EXISTING,
        id: "other",
        title: "Team automation",
        created_by_id: "user-2",
      },
    ];
    renderPage();
    await screen.findByText("Team automation");
    fireEvent.click(screen.getByRole("tab", { name: "Mine" }));
    expect(screen.queryByText("Team automation")).not.toBeInTheDocument();
    expect(screen.getByText("Existing automation")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("tab", { name: "Team" }));
    expect(screen.getByText("Team automation")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "All Runs" })).toHaveAttribute(
      "href",
      "/acme/automations/runs",
    );
  });
  it("searches existing rows while keeping recommendations visible", async () => {
    renderPage();
    await screen.findByText("Existing automation");
    fireEvent.click(screen.getByRole("button", { name: "Search automations" }));
    fireEvent.change(
      screen.getByRole("textbox", { name: "Search automations" }),
      { target: { value: "no match" } },
    );
    expect(screen.queryByText("Existing automation")).not.toBeInTheDocument();
    expect(screen.getByTestId("template-gallery")).toBeInTheDocument();
  });
  it("renders configured tool objects and omits disabled tools", async () => {
    mocks.automations = [
      {
        ...EXISTING,
        tools: {
          memories: {},
          slack_send: { enabled: false },
          mcp_server_ids: ["server-1"],
        },
      },
    ];
    renderPage();
    expect(await screen.findByLabelText("Memories")).toBeInTheDocument();
    expect(screen.queryByTitle("Send to Slack")).not.toBeInTheDocument();
    expect(screen.getByTitle("MCP servers")).toHaveTextContent("1");
  });
});
