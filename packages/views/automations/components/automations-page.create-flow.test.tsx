// @vitest-environment jsdom

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Automation } from "@patchbay/core/types";
import { renderWithI18n } from "../../test/i18n";

const mocks = vi.hoisted(() => ({
  automations: [] as Automation[],
  listError: null as Error | null,
  push: vi.fn(),
}));

vi.mock("@patchbay/core/hooks", () => ({
  useWorkspaceId: () => "ws-test",
}));

vi.mock("@patchbay/core/paths", () => ({
  useWorkspacePaths: () => ({
    automationDetail: (id: string) => `/acme/automations/${id}`,
  }),
}));

vi.mock("../../navigation", () => ({
  useNavigation: () => ({ push: mocks.push }),
  useRowLink: () => () => ({}),
}));

vi.mock("@patchbay/core/workspace/hooks", () => ({
  useActorName: () => ({
    getActorName: () => "Scout",
  }),
}));

vi.mock("@patchbay/core/automations/queries", () => ({
  automationListOptions: () => ({
    queryKey: ["automations", "ws-test"],
    queryFn: async () => {
      if (mocks.listError) throw mocks.listError;
      return mocks.automations;
    },
  }),
}));

vi.mock("@patchbay/core/automations/stores", async (importOriginal) => {
  const actual =
    await importOriginal<typeof import("@patchbay/core/automations/stores")>();
  const state = {
    scope: "all",
    setScope: vi.fn(),
    sortField: "name",
    sortDirection: "asc",
    hiddenColumns: actual.AUTOMATION_DEFAULT_HIDDEN_COLUMNS,
    filters: {
      assignees: [],
      modes: [],
      triggerKinds: [],
      creators: [],
    },
    toggleSort: vi.fn(),
    setSortField: vi.fn(),
    setSortDirection: vi.fn(),
    toggleColumn: vi.fn(),
    toggleFilter: vi.fn(),
    clearFilters: vi.fn(),
  };
  return {
    ...actual,
    useAutomationsViewStore: (selector: (value: typeof state) => unknown) =>
      selector(state),
  };
});

vi.mock("@tanstack/react-virtual", () => ({
  useVirtualizer: ({ count }: { count: number }) => ({
    getVirtualItems: () =>
      Array.from({ length: count }, (_, index) => ({
        index,
        start: index * 48,
        end: (index + 1) * 48,
      })),
    getTotalSize: () => count * 48,
  }),
}));

vi.mock("../../common/actor-avatar", () => ({
  ActorAvatar: () => <span data-testid="actor-avatar" />,
}));

vi.mock("./automation-list-toolbar", () => ({
  actorFilterValue: (type: string, id: string) => `${type}:${id}`,
  AutomationListToolbar: () => <div data-testid="automation-list-toolbar" />,
}));

vi.mock("./automation-list-actions", () => ({
  AutomationBatchToolbar: () => <div data-testid="automation-batch-toolbar" />,
  AutomationRowActions: () => null,
}));

vi.mock("./automation-template-gallery", async (importOriginal) => {
  const actual =
    await importOriginal<typeof import("./automation-template-gallery")>();
  return {
    ...actual,
    AutomationTemplateGallery: ({
      onSelectTemplate,
      persistent,
    }: {
      onSelectTemplate: (template: { id: string }) => void;
      persistent: boolean;
    }) => (
      <div data-testid="template-gallery" data-persistent={persistent}>
        <button
          type="button"
          onClick={() =>
            onSelectTemplate({ id: "find_critical_bugs" } as never)
          }
        >
          Choose recommended automation
        </button>
      </div>
    ),
  };
});

vi.mock("./automation-create-panel", () => ({
  AutomationCreatePanel: () => <div data-testid="automation-create-panel" />,
}));

import { AutomationsPage } from "./automations-page";

const EXISTING_AUTOMATION: Automation = {
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
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return renderWithI18n(
    <QueryClientProvider client={client}>
      <AutomationsPage />
    </QueryClientProvider>,
  );
}

describe("AutomationsPage add flow", () => {
  beforeEach(() => {
    mocks.automations = [EXISTING_AUTOMATION];
    mocks.listError = null;
    mocks.push.mockReset();
  });

  it("keeps recommendations above existing automations and never opens the retired create dialog", async () => {
    renderPage();

    const gallery = await screen.findByTestId("template-gallery");
    const listHeading = screen.getByRole("heading", {
      name: "Your automations",
    });
    expect(gallery).toHaveAttribute("data-persistent", "true");
    expect(
      gallery.compareDocumentPosition(listHeading) &
        Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "Add automation" }));
    expect(screen.queryByTestId("automation-create-panel")).toBeNull();
    expect(screen.getByTestId("template-gallery")).toBeInTheDocument();

    fireEvent.click(
      screen.getByRole("button", { name: "Choose recommended automation" }),
    );
    expect(screen.getByTestId("automation-create-panel")).toBeInTheDocument();
    expect(
      screen.queryByTestId("automation-batch-toolbar"),
    ).not.toBeInTheDocument();
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("opens a blank create panel from the header after the list query fails", async () => {
    mocks.listError = new Error("Automation list unavailable");
    renderPage();

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Automation list unavailable",
    );
    fireEvent.click(screen.getByRole("button", { name: "Add automation" }));
    expect(screen.getByTestId("automation-create-panel")).toBeInTheDocument();
  });
});
