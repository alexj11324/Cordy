// @vitest-environment jsdom

import { describe, expect, it, vi } from "vitest";
import { fireEvent, screen } from "@testing-library/react";
import type { Agent, AgentRuntime } from "@orvilo/core/types";
import { renderWithI18n } from "../../test/i18n";
import { AgentCard, AgentCreateCard } from "./agent-card";
import type { AgentListRow } from "./agents-page";

vi.mock("../../runtimes/components/provider-logo", () => ({
  ProviderLogo: ({ provider }: { provider: string }) => (
    <span data-testid={`provider-${provider}`}>{provider}</span>
  ),
}));

vi.mock("@orvilo/core/runtimes", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@orvilo/core/runtimes")>();
  return {
    ...actual,
    deviceDisplayName: (runtime: AgentRuntime) =>
      runtime.name.match(/\(([^)]+)\)/)?.[1] ?? runtime.name,
    deviceKind: () => "desktop",
    runtimeDisplayLabel: (runtime: AgentRuntime) => runtime.name,
  };
});

function agent(overrides: Partial<Agent> = {}): Agent {
  return {
    id: "agent-1",
    name: "Codex",
    description: "Reviews diffs.",
    model: "gpt-5",
    runtime_id: "rt-1",
    runtime_bound: true,
    owner_id: "user-1",
    visibility: "workspace",
    permission_mode: "public_to",
    invocation_targets: [{ target_type: "workspace", target_id: null }],
    archived_at: null,
    created_at: "2026-01-01T00:00:00Z",
    max_concurrent_tasks: 6,
    ...overrides,
  } as unknown as Agent;
}

function runtime(overrides: Partial<AgentRuntime> = {}): AgentRuntime {
  return {
    id: "rt-1",
    workspace_id: "ws-1",
    daemon_id: "daemon-1",
    name: "Alex MacBook Pro",
    runtime_mode: "local",
    provider: "codex",
    launch_header: "",
    status: "online",
    device_info: "",
    metadata: {},
    owner_id: "user-1",
    visibility: "private",
    last_seen_at: null,
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
    ...overrides,
  };
}

function row(overrides: Partial<AgentListRow> = {}): AgentListRow {
  return {
    agent: agent(),
    runtime: runtime(),
    presence: {
      availability: "online",
      workload: "working",
      runningCount: 3,
      queuedCount: 0,
      capacity: 6,
    },
    activity: null,
    runCount: 4,
    lastActiveDays: 15,
    owner: {
      id: "mem-1",
      workspace_id: "ws-1",
      user_id: "user-1",
      role: "owner",
      created_at: "2026-01-01T00:00:00Z",
      name: "Mira Stone",
      email: "mira@example.com",
      avatar_url: null,
    },
    isOwnedByMe: false,
    canManage: true,
    ...overrides,
  };
}

function renderCard(
  listRow: AgentListRow = row(),
  onOpenSummary = vi.fn(),
  localDaemonId: string | null | undefined = "daemon-1",
  localMachine?: {
    localMachineName?: string | null;
    currentUserId?: string | null;
  },
) {
  return {
    onOpenSummary,
    ...renderWithI18n(
      <AgentCard
        currentUserId={localMachine?.currentUserId}
        localDaemonId={localDaemonId}
        localMachineName={localMachine?.localMachineName}
        onOpenSummary={onOpenSummary}
        row={listRow}
      />,
    ),
  };
}

describe("AgentCard", () => {
  it("maps agent fields onto the Atlas card without deal-specific slots", () => {
    renderCard();

    expect(screen.getByText("Access")).toBeInTheDocument();
    expect(screen.getByText("Workspace")).toBeInTheDocument();
    expect(screen.getByText("Device")).toBeInTheDocument();
    expect(screen.getByText("This machine")).toBeInTheDocument();
    expect(screen.getByText("Concurrency headroom")).toBeInTheDocument();
    expect(screen.getByText("3/6")).toBeInTheDocument();
    expect(screen.getByText("Online")).toBeInTheDocument();
    expect(screen.getByTestId("provider-codex")).toBeInTheDocument();
    expect(screen.queryByText("Risk")).not.toBeInTheDocument();
    expect(screen.queryByText(/left/)).not.toBeInTheDocument();
    expect(screen.getByText("Mira Stone")).toBeInTheDocument();
    expect(screen.queryByText("Alex MacBook Pro")).not.toBeInTheDocument();
  });

  it("keeps the create card in the same fixed gallery geometry", () => {
    const { container } = renderCard();
    const agentCard = container.querySelector('[data-slot="card"]');

    renderWithI18n(
      <AgentCreateCard ariaLabel="New agent" onClick={vi.fn()} />,
    );

    const createCard = screen.getByTestId("new-agent-card").closest(
      '[data-slot="card"]',
    );
    expect(agentCard).toHaveClass("h-[13.75rem]");
    expect(createCard).toHaveClass("h-[13.75rem]");
  });

  it("shows full green headroom when idle", () => {
    const { container } = renderCard(
      row({
        presence: {
          availability: "online",
          workload: "idle",
          runningCount: 0,
          queuedCount: 0,
          capacity: 6,
        },
      }),
    );

    expect(screen.getByText("6/6")).toBeInTheDocument();
    expect(container.querySelector('[data-slot="progress"]')?.className).toContain(
      "bg-success",
    );
  });

  it("keeps a red sliver when all concurrency is occupied", () => {
    const { container } = renderCard(
      row({
        presence: {
          availability: "online",
          workload: "working",
          runningCount: 6,
          queuedCount: 0,
          capacity: 6,
        },
      }),
    );

    expect(screen.getByText("0/6")).toBeInTheDocument();
    const indicator = container.querySelector('[data-slot="progress-indicator"]');
    expect(container.querySelector('[data-slot="progress"]')?.className).toContain(
      "bg-destructive",
    );
    expect(indicator).toHaveStyle({ width: "2%" });
  });

  it("uses amber for an unstable connection", () => {
    renderCard(
      row({
        presence: {
          availability: "unstable",
          workload: "idle",
          runningCount: 0,
          queuedCount: 0,
          capacity: 6,
        },
      }),
    );

    expect(screen.getByText("Unstable connection")).toBeInTheDocument();
  });

  it("uses red for an unavailable device", () => {
    renderCard(
      row({
        runtime: null,
        presence: null,
      }),
    );

    expect(screen.getByText("Offline")).toBeInTheDocument();
  });

  it("uses the remote runtime name as the device value", () => {
    renderCard(
      row({
        runtime: runtime({
          runtime_mode: "cloud",
          name: "Cloud Sandbox",
          provider: "cursor",
        }),
      }),
    );

    expect(screen.getAllByText("Cloud Sandbox").length).toBeGreaterThanOrEqual(1);
    expect(screen.queryByText("This machine")).not.toBeInTheDocument();
    expect(screen.getByTestId("provider-cursor")).toBeInTheDocument();
  });

  it("distinguishes another local daemon from this machine", () => {
    renderCard(
      row({
        runtime: runtime({
          daemon_id: "daemon-2",
          name: "Codex (Other Mac)",
        }),
      }),
      vi.fn(),
      "daemon-1",
    );

    expect(screen.getByText("Other Mac")).toBeInTheDocument();
    expect(screen.queryByText("This machine")).not.toBeInTheDocument();
  });

  it("still labels this machine after a new app mints a different daemon UUID", () => {
    renderCard(
      row({
        runtime: runtime({
          daemon_id: "daemon-old",
          name: "Claude (Mac)",
          device_info: "Mac · darwin-arm64",
        }),
      }),
      vi.fn(),
      "daemon-new",
      { localMachineName: "Mac", currentUserId: "user-1" },
    );

    expect(screen.getByText("This machine")).toBeInTheDocument();
    expect(screen.queryByText("Mac")).not.toBeInTheDocument();
  });

  it("uses the owner name and a colored ReUI icon fallback", () => {
    const { container } = renderCard(
      row({
        isOwnedByMe: true,
        owner: {
          id: "mem-1",
          workspace_id: "ws-1",
          user_id: "user-1",
          role: "owner",
          created_at: "2026-01-01T00:00:00Z",
          name: "Alex Jiang",
          email: "alex@example.com",
          avatar_url: null,
        },
      }),
    );

    expect(screen.getByText("Alex Jiang")).toBeInTheDocument();
    expect(screen.queryByText("你")).not.toBeInTheDocument();
    expect(container.querySelector('[data-slot="avatar-fallback"]')).toHaveClass(
      "bg-primary/10",
      "text-primary",
    );
    expect(
      container.querySelector('[data-slot="avatar-fallback"] svg'),
    ).toBeInTheDocument();
  });

  it("opens the overlay inspector from the card title", () => {
    const { onOpenSummary } = renderCard();

    fireEvent.click(screen.getByRole("button", { name: "Codex" }));

    expect(onOpenSummary).toHaveBeenCalledTimes(1);
  });
});
