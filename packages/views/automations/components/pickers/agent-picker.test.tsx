import { describe, expect, it, vi } from "vitest";
import type { ReactNode } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { screen, within } from "@testing-library/react";
import { renderWithI18n } from "../../../test/i18n";
import { AgentPicker } from "./agent-picker";

const fixtures = vi.hoisted(() => ({
  agents: [
    {
      id: "agent-archived",
      name: "Retired Scout",
      archived_at: "2026-09-07T12:00:00Z",
      runtime_id: "runtime-archived",
    },
    {
      id: "agent-active",
      name: "Scout",
      archived_at: null,
      runtime_id: "runtime-active",
    },
  ],
  teams: [
    {
      id: "team-1",
      name: "Review Team",
      leader_id: "agent-active",
      archived_at: null,
    },
  ],
}));

vi.mock("@patchbay/core/hooks", () => ({ useWorkspaceId: () => "workspace-1" }));

vi.mock("@patchbay/core/agents", () => ({
  isAgentRuntimeBound: (agent: { runtime_id?: string }) => Boolean(agent.runtime_id),
}));

vi.mock("@patchbay/core/workspace/queries", () => ({
  agentListOptions: (workspaceId: string) => ({
    queryKey: ["agents", workspaceId],
    queryFn: async () => fixtures.agents,
  }),
  teamListOptions: (workspaceId: string) => ({
    queryKey: ["teams", workspaceId],
    queryFn: async () => fixtures.teams,
  }),
}));

vi.mock("../../../common/actor-avatar", () => ({
  ActorAvatar: ({ actorId }: { actorId: string }) => <span data-testid={`avatar-${actorId}`} />,
}));

vi.mock("../../../issues/components/pickers/property-picker", () => ({
  PropertyPicker: ({ trigger, children }: { trigger: ReactNode; children: ReactNode }) => (
    <div>
      <div data-testid="picker-trigger">{trigger}</div>
      <div data-testid="picker-options">{children}</div>
    </div>
  ),
  PickerItem: ({
    children,
    disabled,
  }: {
    children: ReactNode;
    disabled?: boolean;
  }) => <button type="button" disabled={disabled}>{children}</button>,
  PickerSection: ({ label, children }: { label: string; children: ReactNode }) => (
    <section aria-label={label}>{children}</section>
  ),
  PickerEmpty: () => <div>No results</div>,
}));

describe("AgentPicker", () => {
  it("shows an archived current agent without offering it as a new choice", async () => {
    const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });

    renderWithI18n(
      <QueryClientProvider client={queryClient}>
        <AgentPicker
          assignee={{ type: "agent", id: "agent-archived" }}
          onChange={vi.fn()}
        />
      </QueryClientProvider>,
    );

    await screen.findByText("Retired Scout");
    expect(screen.getByTestId("picker-trigger")).toHaveTextContent("Retired Scout");
    expect(screen.getByTestId("picker-trigger")).toHaveTextContent("Archived");
    const options = within(screen.getByTestId("picker-options"));
    expect(options.getByTestId("avatar-agent-active")).toBeInTheDocument();
    expect(options.queryByTestId("avatar-agent-archived")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Review Team/ })).toBeInTheDocument();
  });
});
