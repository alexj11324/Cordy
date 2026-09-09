// @vitest-environment jsdom

import React from "react";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { Agent } from "@orvilo/core/types";
import { I18nProvider } from "@orvilo/core/i18n/react";
import enCommon from "../../locales/en/common.json";
import enAgents from "../../locales/en/agents.json";

const TEST_RESOURCES = { en: { common: enCommon, agents: enAgents } };

vi.mock("@orvilo/core/paths", () => ({
  useWorkspacePaths: () => ({
    agentDetail: (id: string) => `/acme/agents/${id}`,
    newAgentManual: () => "/acme/agents/new/manual",
  }),
}));

vi.mock("@orvilo/core/agents", () => ({
  effectiveAccessScope: () => "workspace",
  isAgentRuntimeBound: (
    candidate: Pick<Agent, "runtime_id" | "runtime_bound">,
  ) =>
    candidate.runtime_bound !== false &&
    (candidate.runtime_id ?? "").trim().length > 0,
  providerSupportsMcpConfig: () => true,
}));

vi.mock("../../common/actor-avatar", () => ({
  ActorAvatar: ({ actorId }: { actorId: string }) => (
    <span data-testid={`avatar-${actorId}`} />
  ),
}));

vi.mock("./agent-presence-indicator", () => ({
  AgentPresenceIndicator: () => <span>Online · Idle</span>,
}));

vi.mock("../../navigation", () => ({
  AppLink: ({
    children,
    href,
    ...props
  }: React.AnchorHTMLAttributes<HTMLAnchorElement>) => (
    <a href={href} {...props}>
      {children}
    </a>
  ),
}));

import { AgentProfilePanel } from "./agent-profile-panel";
import type { AgentListRow } from "./agents-page";

const agent: Agent = {
  id: "agent-1",
  workspace_id: "ws-1",
  runtime_id: "runtime-1",
  name: "Review Agent",
  description: "Reviews frontend pull requests.",
  instructions: "Review the diff and call out risky changes.",
  avatar_url: null,
  runtime_mode: "local",
  runtime_config: {},
  custom_args: [],
  visibility: "workspace",
  permission_mode: "public_to",
  invocation_targets: [{ target_type: "workspace", target_id: null }],
  status: "idle",
  max_concurrent_tasks: 2,
  model: "claude-opus-4-8",
  owner_id: "user-1",
  skills: [{ id: "skill-1", name: "Frontend review", description: "" }],
  created_at: "2026-05-28T00:00:00Z",
  updated_at: "2026-05-28T00:00:00Z",
  archived_at: null,
  archived_by: null,
};

const row = {
  agent,
  runtime: {
    id: "runtime-1",
    workspace_id: "ws-1",
    daemon_id: null,
    name: "Codex Host",
    runtime_mode: "local",
    provider: "codex",
    launch_header: "",
    status: "online",
    device_info: "",
    metadata: {},
    owner_id: "user-1",
    visibility: "private",
    last_seen_at: null,
    created_at: "2026-05-28T00:00:00Z",
    updated_at: "2026-05-28T00:00:00Z",
  },
  presence: {
    availability: "online",
    workload: "idle",
    runningCount: 0,
    queuedCount: 0,
    capacity: 2,
  },
  activity: null,
  runCount: 4,
  lastActiveDays: 0,
  owner: { user_id: "user-1", name: "Alex", email: "alex@example.com" },
  isOwnedByMe: true,
  canManage: true,
} as AgentListRow;

function renderPanel(onClose = vi.fn(), panelRow = row) {
  return {
    onClose,
    ...render(
      <I18nProvider locale="en" resources={TEST_RESOURCES}>
        <AgentProfilePanel row={panelRow} onClose={onClose} />
      </I18nProvider>,
    ),
  };
}

describe("AgentProfilePanel", () => {
  it("portals the right drawer outside the gallery layout", () => {
    const { container } = renderPanel();
    const dialog = screen.getByRole("dialog", { name: "Profile" });

    expect(container).not.toContainElement(dialog);
    expect(dialog).toHaveAttribute("data-side", "right");
    expect(dialog).toHaveAttribute("aria-modal", "true");
  });

  it("closes the drawer with Escape", async () => {
    const { onClose } = renderPanel();
    fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
    await waitFor(() => expect(onClose).toHaveBeenCalledTimes(1));
  });

  it("hides Edit from read-only viewers and keeps unsupported destinations as text", () => {
    renderPanel(vi.fn(), { ...row, canManage: false });
    expect(screen.queryByRole("button", { name: "Edit" })).not.toBeInTheDocument();
    expect(screen.getByText("Agent instructions").closest("a")).toBeNull();
    expect(screen.getByRole("dialog").querySelector('a[href*="view=instructions"], a[href*="view=skills"], a[href*="view=mcp_config"], a[href*="view=work"]')).toBeNull();
  });

  it("uses the Buzz-style profile hero and grouped info rows", () => {
    renderPanel();

    expect(screen.getByRole("heading", { name: "Profile" })).toBeInTheDocument();
    expect(screen.getByText("Review Agent")).toBeInTheDocument();
    expect(screen.getByText("Agent instructions")).toBeInTheDocument();
    expect(screen.getByText("Agent ID")).toBeInTheDocument();
    expect(screen.getByText("Duplicate")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Edit" })).toHaveAttribute(
      "href",
      "/acme/agents/agent-1?view=general",
    );
  });

  it("switches the segmented profile tabs without leaving the panel", () => {
    renderPanel();

    fireEvent.click(screen.getByRole("tab", { name: "Harness" }));

    expect(screen.getByText("Codex Host")).toBeInTheDocument();
    expect(screen.getByText("claude-opus-4-8")).toBeInTheDocument();
  });

  it("closes from the header control", () => {
    const { onClose } = renderPanel();

    fireEvent.click(screen.getByRole("button", { name: "Close profile" }));

    expect(onClose).toHaveBeenCalledTimes(1);
  });
});
