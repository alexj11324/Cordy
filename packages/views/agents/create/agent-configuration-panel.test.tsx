// @vitest-environment jsdom

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, screen } from "@testing-library/react";
import { EMPTY_AGENT_DRAFT } from "@orvilo/core/agents";
import { configStore } from "@orvilo/core/config";
import type { RuntimeDevice } from "@orvilo/core/types";
import { renderWithI18n } from "../../test/i18n";
import enAgents from "../../locales/en/agents.json";
import { AgentConfigurationPanel } from "./agent-configuration-panel";

vi.mock("../components/model-dropdown", () => ({
  ModelDropdown: () => <div data-testid="model-dropdown" />,
}));

vi.mock("../../common/actor-avatar", () => ({
  ActorAvatar: () => null,
}));

const RUNTIME: RuntimeDevice = {
  id: "rt-1",
  workspace_id: "ws-1",
  daemon_id: null,
  name: "Test Runtime",
  runtime_mode: "local",
  provider: "claude",
  launch_header: "",
  status: "online",
  device_info: "host.local",
  metadata: {},
  owner_id: "user-1",
  visibility: "private",
  last_seen_at: "2026-04-27T11:59:50Z",
  created_at: "2026-04-01T00:00:00Z",
  updated_at: "2026-04-01T00:00:00Z",
};

function renderPanel(startersSupported = false, showConversationStarters = true) {
  configStore.getState().setAgentConversationStartersSupported(startersSupported);
  return renderWithI18n(
    <AgentConfigurationPanel
      showConversationStarters={showConversationStarters}
      draft={{ ...EMPTY_AGENT_DRAFT, name: "Draft agent", runtimeId: RUNTIME.id }}
      onChange={vi.fn()}
      runtimes={[RUNTIME]}
      runtimesLoading={false}
      members={[]}
      currentUserId="user-1"
      nameError={null}
      onNameChange={vi.fn()}
    />,
  );
}

describe("AgentConfigurationPanel", () => {
  beforeEach(() => {
    configStore.getState().setAgentConversationStartersSupported(false);
  });

  afterEach(() => {
    cleanup();
    configStore.getState().setAgentConversationStartersSupported(false);
  });

  it("keeps identity, execution, and access, and drops description, instructions, and skills", () => {
    renderPanel();

    expect(screen.getByText(enAgents.creation_studio.sections.identity)).toBeInTheDocument();
    expect(screen.getByText(enAgents.creation_studio.sections.execution)).toBeInTheDocument();
    expect(screen.getByText(enAgents.creation_studio.sections.access)).toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: enAgents.create_dialog.name_label })).toBeInTheDocument();
    expect(screen.getByTestId("model-dropdown")).toBeInTheDocument();
    expect(
      screen.queryByLabelText(enAgents.create_dialog.avatar.change_aria),
    ).not.toBeInTheDocument();

    expect(screen.queryByLabelText(enAgents.create_dialog.description_label)).toBeNull();
    expect(screen.queryByText(enAgents.create_dialog.description_label, { exact: true })).toBeNull();
    expect(screen.queryByLabelText(enAgents.create_dialog.instructions.label)).toBeNull();
    expect(screen.queryByText(enAgents.creation_studio.sections.behavior)).toBeNull();
    expect(screen.queryByText(enAgents.create_dialog.skills_section.label, { exact: true })).toBeNull();
    expect(
      screen.queryByText(enAgents.create_dialog.skills_section.placeholder),
    ).toBeNull();
  });

  it("does not add prompt fields to creation when conversation starters are supported", () => {
    renderPanel(true, false);

    expect(screen.queryByText(enAgents.conversation_starters.label)).not.toBeInTheDocument();
    expect(screen.queryByText(enAgents.creation_studio.sections.behavior)).toBeNull();
  });

  it("preserves conversation starter editing for existing builder sessions", () => {
    renderPanel(true);
    expect(screen.getByText(enAgents.conversation_starters.label)).toBeInTheDocument();
  });
});
