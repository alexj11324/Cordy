// @vitest-environment jsdom

import { describe, it, expect, vi, beforeEach } from "vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { ComponentProps } from "react";
import type { Agent, AgentRuntime, MemberWithUser } from "@orvilo/core/types";
import { I18nProvider } from "@orvilo/core/i18n/react";
import enCommon from "../../locales/en/common.json";
import enAgents from "../../locales/en/agents.json";
import {
  NavigationProvider,
  type NavigationAdapter,
} from "../../navigation";
import { AgentIdentityCard } from "./agent-identity-card";

const TEST_RESOURCES = { en: { common: enCommon, agents: enAgents } };

const catalogRef = vi.hoisted(() => ({
  models: [] as Array<{
    id: string;
    label: string;
    service_tiers?: Array<{ id: string; name: string }>;
    thinking?: { supported_levels: Array<{ value: string; label: string }> };
  }>,
}));

vi.mock("@orvilo/core/runtimes", async (importOriginal) => {
  const actual =
    await importOriginal<typeof import("@orvilo/core/runtimes")>();
  return {
    ...actual,
    runtimeModelsOptions: (id: string | null) => ({
      queryKey: ["models", id],
      enabled: Boolean(id),
      queryFn: async () => ({
        supported: true,
        models: catalogRef.models,
      }),
    }),
  };
});

vi.mock("../../common/actor-avatar", () => ({
  ActorAvatar: ({
    actorType,
    actorId,
  }: {
    actorType: string;
    actorId: string;
  }) => <div data-testid={`avatar-${actorType}-${actorId}`} />,
}));

const agent: Agent = {
  id: "agent-1",
  workspace_id: "ws-1",
  runtime_id: "runtime-1",
  name: "Model Picker Verification",
  description: "Should never appear on the identity card",
  instructions: "Nor should instructions",
  avatar_url: null,
  runtime_mode: "local",
  runtime_config: {},
  custom_args: [],
  visibility: "private",
  permission_mode: "private",
  invocation_targets: [],
  status: "idle",
  max_concurrent_tasks: 5,
  model: "gpt-5.6-sol",
  thinking_level: "low",
  service_tier: "default",
  owner_id: "user-1",
  skills: [],
  created_at: "2026-05-28T00:00:00Z",
  updated_at: "2026-05-28T00:00:00Z",
  archived_at: null,
  archived_by: null,
};

const runtime: AgentRuntime = {
  id: "runtime-1",
  workspace_id: "ws-1",
  daemon_id: null,
  name: "Antigravity (Mac)",
  runtime_mode: "local",
  provider: "antigravity",
  launch_header: "",
  status: "online",
  device_info: "",
  metadata: {},
  owner_id: null,
  visibility: "private",
  last_seen_at: null,
  created_at: "2026-05-28T00:00:00Z",
  updated_at: "2026-05-28T00:00:00Z",
};

const owner: MemberWithUser = {
  id: "member-1",
  workspace_id: "ws-1",
  user_id: "user-1",
  role: "owner",
  created_at: "2026-05-28T00:00:00Z",
  name: "dev",
  email: "dev@example.com",
  avatar_url: null,
};

function renderCard(
  overrides: Partial<ComponentProps<typeof AgentIdentityCard>> = {},
) {
  const navigation: NavigationAdapter = {
    push: vi.fn(),
    replace: vi.fn(),
    back: vi.fn(),
    pathname: "/acme/agents/agent-1",
    searchParams: new URLSearchParams(),
    hash: "",
    getShareableUrl: (path) => path,
  };
  const onUpdate = overrides.onUpdate ?? vi.fn().mockResolvedValue(undefined);
  const onDm = overrides.onDm ?? vi.fn();
  const props: ComponentProps<typeof AgentIdentityCard> = {
    agent,
    runtime,
    owner,
    presence: {
      availability: "online",
      workload: "idle",
      runningCount: 0,
      queuedCount: 0,
      capacity: 5,
    },
    canEdit: true,
    dmPending: false,
    dmHref: "/acme/chat?agent=agent-1",
    onDm,
    onUpdate,
    ...overrides,
  };
  const renderTree = (cardProps: ComponentProps<typeof AgentIdentityCard>) => (
    <I18nProvider locale="en" resources={TEST_RESOURCES}>
      <NavigationProvider value={navigation}>
        <QueryClientProvider
          client={
            new QueryClient({
              defaultOptions: { queries: { retry: false } },
            })
          }
        >
          <AgentIdentityCard {...cardProps} />
        </QueryClientProvider>
      </NavigationProvider>
    </I18nProvider>
  );
  const view = render(renderTree(props));
  return {
    onUpdate,
    onDm,
    rerenderCard: (
      next: Partial<ComponentProps<typeof AgentIdentityCard>>,
    ) => view.rerender(renderTree({ ...props, ...next })),
  };
}

describe("AgentIdentityCard", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    catalogRef.models = [];
  });

  it("puts provider identity, name, model, and one message action in the card", () => {
    renderCard();

    expect(screen.getByRole("heading", { name: "Model Picker Verification" })).toBeInTheDocument();
    expect(screen.getByTestId("agent-name-value")).toHaveTextContent(
      "Model Picker Verification",
    );
    expect(screen.getByText("gpt-5.6-sol · low · Standard")).toBeInTheDocument();
    expect(screen.queryByText("Online")).not.toBeInTheDocument();
    expect(screen.queryByText("Idle")).not.toBeInTheDocument();
    expect(document.querySelector('[data-slot="avatar-badge"]')).toHaveClass("bg-success");
    expect(screen.getByText("Mac")).toBeInTheDocument();
    expect(screen.getByText("dev")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Send message" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Edit agent" })).toBeInTheDocument();
    expect(screen.queryByLabelText("Agent actions")).not.toBeInTheDocument();
  });

  it("does not render description or instructions", () => {
    renderCard();
    expect(
      screen.queryByText("Should never appear on the identity card"),
    ).not.toBeInTheDocument();
    expect(screen.queryByText("Nor should instructions")).not.toBeInTheDocument();
  });

  it("renders compact page identity once with owner, access, and message actions", () => {
    renderCard({ breadcrumbHref: "/acme/agents" });

    expect(screen.getAllByRole("heading", { name: agent.name })).toHaveLength(1);
    expect(screen.getByRole("link", { name: "Agents" })).toHaveAttribute("href", "/acme/agents");
    expect(screen.queryByRole("complementary")).not.toBeInTheDocument();
    expect(screen.queryByText("gpt-5.6-sol · low · Standard")).not.toBeInTheDocument();
    expect(screen.getByText("dev")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Send message" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Edit agent" })).toBeInTheDocument();
  });

  it.each([undefined, "/acme/agents"])("lets Edit change the name and saves on Done (%s)", async (breadcrumbHref) => {
    const user = userEvent.setup();
    const { onUpdate } = renderCard({ breadcrumbHref });

    await user.click(screen.getByRole("button", { name: "Edit agent" }));
    expect(
      document.querySelector('[data-agent-identity="assigned"]'),
    ).toBeInTheDocument();
    const input = screen.getByRole("textbox", { name: "Name" });
    await user.clear(input);
    await user.type(input, "Renamed Agent");
    await user.click(screen.getByRole("button", { name: "Finish editing agent" }));

    await waitFor(() =>
      expect(onUpdate).toHaveBeenCalledWith("agent-1", { name: "Renamed Agent" }),
    );
  });

  it("retains the draft when rename fails", async () => {
    const user = userEvent.setup();
    const onUpdate = vi.fn().mockRejectedValue(new Error("save failed"));
    renderCard({ onUpdate });

    await user.click(screen.getByRole("button", { name: "Edit agent" }));
    const input = screen.getByRole("textbox", { name: "Name" });
    await user.clear(input);
    await user.type(input, "Broken");
    await user.click(screen.getByRole("button", { name: "Finish editing agent" }));

    await waitFor(() =>
      expect(screen.getByRole("textbox", { name: "Name" })).toHaveValue(
        "Broken",
      ),
    );
    expect(
      screen.getByRole("button", { name: "Finish editing agent" }),
    ).toBeInTheDocument();
  });

  it("keeps the rename draft open through an optimistic name update", async () => {
    const user = userEvent.setup();
    let rejectSave: ((reason?: unknown) => void) | undefined;
    const onUpdate = vi.fn(
      () =>
        new Promise<void>((_resolve, reject) => {
          rejectSave = reject;
        }),
    );
    const { rerenderCard } = renderCard({ onUpdate });

    await user.click(screen.getByRole("button", { name: "Edit agent" }));
    const input = screen.getByRole("textbox", { name: "Name" });
    await user.clear(input);
    await user.type(input, "Optimistic Name");
    await user.click(screen.getByRole("button", { name: "Finish editing agent" }));

    rerenderCard({ agent: { ...agent, name: "Optimistic Name" } });
    expect(screen.getByRole("textbox", { name: "Name" })).toHaveValue(
      "Optimistic Name",
    );

    rejectSave?.(new Error("save failed"));
    rerenderCard({ agent });
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: "Finish editing agent" }),
      ).toBeEnabled(),
    );
    expect(screen.getByRole("textbox", { name: "Name" })).toHaveValue("Optimistic Name");
  });

  it("hides Edit when the caller cannot manage the agent", () => {
    renderCard({ canEdit: false });
    expect(screen.queryByRole("button", { name: "Edit agent" })).toBeNull();
  });

  it("shows Claude Fast by catalog name instead of the stored id true", async () => {
    catalogRef.models = [
      {
        id: "claude-opus-5",
        label: "Claude Opus 5",
        service_tiers: [{ id: "true", name: "Fast" }],
        thinking: {
          supported_levels: [{ value: "low", label: "Low" }],
        },
      },
    ];
    renderCard({
      agent: {
        ...agent,
        model: "claude-opus-5",
        thinking_level: "low",
        service_tier: "true",
      },
      runtime: { ...runtime, provider: "claude", name: "Claude (Mac)" },
    });
    expect(
      await screen.findByText("Claude Opus 5 · Low · Fast"),
    ).toBeInTheDocument();
    expect(screen.queryByText(/· true$/)).toBeNull();
  });
});
