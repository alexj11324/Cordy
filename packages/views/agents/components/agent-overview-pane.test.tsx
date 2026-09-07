// @vitest-environment jsdom

import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { Agent, AgentRuntime } from "@orvilo/core/types";
import { I18nProvider } from "@orvilo/core/i18n/react";
import enCommon from "../../locales/en/common.json";
import enAgents from "../../locales/en/agents.json";
import {
  NavigationProvider,
  type NavigationAdapter,
} from "../../navigation";

const TEST_RESOURCES = { en: { common: enCommon, agents: enAgents } };

vi.mock("./agent-access-settings", () => ({
  AgentAccessSettings: () => <div>agent-access-settings</div>,
}));
vi.mock("./tabs/env-tab", () => ({
  EnvTab: () => <div>env-tab</div>,
}));
vi.mock("./tabs/custom-args-tab", () => ({
  CustomArgsTab: () => <div>custom-args-tab</div>,
}));
vi.mock("./tabs/integrations-tab", () => ({
  IntegrationsTab: () => <div>integrations-tab</div>,
}));
vi.mock("./agent-detail-inspector", () => ({
  AgentDetailInspector: () => <div>agent-detail-inspector</div>,
}));

const larkListingRef = vi.hoisted(() => ({
  current: { installations: [] as unknown[], configured: false },
}));
const slackListingRef = vi.hoisted(() => ({
  current: { installations: [] as unknown[], configured: false },
}));
const telegramListingRef = vi.hoisted(() => ({
  current: { installations: [] as unknown[], configured: false },
}));
vi.mock("@orvilo/core/hooks", () => ({
  useWorkspaceId: () => "ws-1",
}));
vi.mock("@orvilo/core/lark", () => ({
  larkInstallationsOptions: () => ({
    queryKey: ["lark", "installations"],
    queryFn: () => Promise.resolve(larkListingRef.current),
  }),
}));
vi.mock("@orvilo/core/slack", () => ({
  slackInstallationsOptions: () => ({
    queryKey: ["slack", "installations"],
    queryFn: () => Promise.resolve(slackListingRef.current),
  }),
}));
vi.mock("@orvilo/core/telegram", () => ({
  telegramInstallationsOptions: () => ({
    queryKey: ["telegram", "installations"],
    queryFn: () => Promise.resolve(telegramListingRef.current),
  }),
}));
vi.mock("@orvilo/core/dingtalk", () => ({
  dingtalkInstallationsOptions: () => ({
    queryKey: ["dingtalk", "installations"],
    queryFn: () => Promise.resolve({ installations: [], configured: false }),
  }),
}));
vi.mock("@orvilo/core/wecom", () => ({
  wecomInstallationsOptions: () => ({
    queryKey: ["wecom", "installations"],
    queryFn: () => Promise.resolve({ installations: [], configured: false }),
  }),
}));

import { AgentOverviewPane } from "./agent-overview-pane";

const baseAgent: Agent = {
  id: "agent-1",
  workspace_id: "ws-1",
  runtime_id: "runtime-1",
  name: "Agent",
  description: "",
  instructions: "",
  avatar_url: null,
  runtime_mode: "local",
  runtime_config: {},
  custom_args: [],
  visibility: "workspace",
  permission_mode: "public_to",
  invocation_targets: [{ target_type: "workspace", target_id: null }],
  status: "idle",
  max_concurrent_tasks: 1,
  model: "",
  owner_id: "user-1",
  skills: [],
  created_at: "2026-05-28T00:00:00Z",
  updated_at: "2026-05-28T00:00:00Z",
  archived_at: null,
  archived_by: null,
};

function makeRuntime(provider: string): AgentRuntime {
  return {
    id: "runtime-1",
    workspace_id: "ws-1",
    daemon_id: null,
    name: "Runtime",
    runtime_mode: "local",
    provider,
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
}

function renderPane(
  runtimes: AgentRuntime[],
  { canEdit = true }: { canEdit?: boolean } = {},
) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  const navigation: NavigationAdapter = {
    push: vi.fn(),
    replace: vi.fn(),
    back: vi.fn(),
    pathname: "/acme/agents/agent-1",
    searchParams: new URLSearchParams(),
    hash: "",
    getShareableUrl: (path) => path,
  };
  return render(
    <I18nProvider locale="en" resources={TEST_RESOURCES}>
      <NavigationProvider value={navigation}>
        <QueryClientProvider client={queryClient}>
          <AgentOverviewPane
            agent={baseAgent}
            runtime={runtimes[0] ?? null}
            owner={null}
            runtimes={runtimes}
            members={[]}
            onUpdate={vi.fn().mockResolvedValue(undefined)}
            canEdit={canEdit}
          />
        </QueryClientProvider>
      </NavigationProvider>
    </I18nProvider>,
  );
}

beforeEach(() => {
  larkListingRef.current = { installations: [], configured: false };
  slackListingRef.current = { installations: [], configured: false };
  telegramListingRef.current = { installations: [], configured: false };
});

describe("AgentOverviewPane removed agent-local tabs", () => {
  it("does not keep Overview, Work, Capabilities, Skills, or per-agent MCP", () => {
    renderPane([makeRuntime("codex")]);
    expect(screen.queryByRole("tab", { name: /^Overview$/i })).not.toBeInTheDocument();
    expect(screen.queryByRole("tab", { name: /^Work$/i })).not.toBeInTheDocument();
    expect(screen.queryByRole("tab", { name: /^Capabilities$/i })).not.toBeInTheDocument();
    expect(screen.queryByRole("tab", { name: /^Skills$/i })).not.toBeInTheDocument();
    expect(screen.queryByRole("tab", { name: /^MCP$/i })).not.toBeInTheDocument();
    expect(screen.queryByRole("tab", { name: /^Instructions$/i })).not.toBeInTheDocument();
    expect(screen.queryByRole("tab", { name: /^MCP Apps$/i })).not.toBeInTheDocument();
  });
});

describe("AgentOverviewPane Integrations tab visibility", () => {
  it("shows the Integrations tab once the deployment has Lark configured", async () => {
    larkListingRef.current = { installations: [], configured: true };
    renderPane([makeRuntime("claude")]);
    expect(
      await screen.findByRole("tab", { name: /^Integrations$/i }),
    ).toBeInTheDocument();
  });

  it("shows the Integrations tab when only Slack is configured (Lark off)", async () => {
    slackListingRef.current = { installations: [], configured: true };
    renderPane([makeRuntime("claude")]);
    expect(
      await screen.findByRole("tab", { name: /^Integrations$/i }),
    ).toBeInTheDocument();
  });

  it("shows the Integrations tab when only Telegram is configured", async () => {
    telegramListingRef.current = { installations: [], configured: true };
    renderPane([makeRuntime("claude")]);
    expect(
      await screen.findByRole("tab", { name: /^Integrations$/i }),
    ).toBeInTheDocument();
  });

  it("hides the Integrations tab when no channel integration is configured", () => {
    renderPane([makeRuntime("claude")]);
    expect(
      screen.queryByRole("tab", { name: /^Integrations$/i }),
    ).not.toBeInTheDocument();
  });
});

describe("AgentOverviewPane Settings navigation", () => {
  it("gives Access its own settings tab", () => {
    renderPane([makeRuntime("claude")]);
    expect(screen.getByRole("tab", { name: /^Access$/i })).toBeInTheDocument();
  });
});

describe("AgentOverviewPane Environment tab visibility", () => {
  it("shows the Environment tab to someone who can manage the agent", () => {
    renderPane([makeRuntime("claude")]);
    expect(
      screen.getByRole("tab", { name: /^Environment$/i }),
    ).toBeInTheDocument();
  });

  it("hides the Environment tab from users who cannot manage the agent", () => {
    renderPane([makeRuntime("claude")], { canEdit: false });
    expect(
      screen.queryByRole("tab", { name: /^Environment$/i }),
    ).not.toBeInTheDocument();
  });
});
