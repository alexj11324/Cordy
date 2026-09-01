// @vitest-environment jsdom

import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { I18nProvider } from "@patchbay/core/i18n/react";
import type { Agent, AgentRuntime } from "@patchbay/core/types";
import enChat from "../../locales/en/chat.json";
import enIssues from "../../locales/en/issues.json";

const updateAgent = vi.hoisted(() => vi.fn());

vi.mock("@tanstack/react-query", () => ({
  useQuery: () => ({
    data: {
      models: [],
      supported: true,
      session_modes: [{ value: "auto", label: "Approve for me", kind: "auto_review" }],
    },
  }),
  useQueryClient: () => ({}),
  queryOptions: (options: unknown) => options,
}));

vi.mock("@patchbay/core/api", () => ({
  api: { updateAgent },
}));

vi.mock("@patchbay/core/hooks", () => ({
  useWorkspaceId: () => "ws-1",
}));

vi.mock("@patchbay/core/paths", () => ({
  useCurrentWorkspace: () => ({
    repos: [{ url: "https://github.com/octocat/hello-world" }],
  }),
}));

vi.mock("@patchbay/core/workspace/queries", () => ({
  cacheAgentResponse: vi.fn(),
}));

import { AgentComposerControlBar } from "./agent-composer-control-bar";

const TEST_RESOURCES = { en: { chat: enChat, issues: enIssues } };

const agent = {
  id: "agent-1",
  name: "Lambda",
  session_mode: "",
} as unknown as Agent;

const runtime = {
  id: "rt-1",
  name: "Studio",
  custom_name: "Desk",
  status: "online",
  provider: "codex",
} as unknown as AgentRuntime;

describe("AgentComposerControlBar", () => {
  beforeEach(() => {
    updateAgent.mockReset();
  });

  it("shows the bound device, workspace repo, and session-mode picker without inventing git refs", () => {
    render(
      <I18nProvider locale="en" resources={TEST_RESOURCES}>
        <AgentComposerControlBar agent={agent} runtime={runtime} canEdit />
      </I18nProvider>,
    );

    expect(screen.getByText("Desk")).toBeInTheDocument();
    expect(screen.getByText("octocat/hello-world")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Session mode/ })).toBeInTheDocument();
    expect(screen.queryByText(/main/)).not.toBeInTheDocument();
    expect(screen.queryByText(/#\d+/)).not.toBeInTheDocument();
  });

  it("falls back to unknown device when no runtime is bound", () => {
    render(
      <I18nProvider locale="en" resources={TEST_RESOURCES}>
        <AgentComposerControlBar agent={agent} runtime={null} canEdit />
      </I18nProvider>,
    );
    expect(screen.getByText("Unknown device")).toBeInTheDocument();
  });
});
