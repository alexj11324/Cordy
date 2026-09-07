// @vitest-environment jsdom

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, cleanup, fireEvent, screen, waitFor } from "@testing-library/react";
import type {
  Agent,
  AgentRuntime,
  RuntimeModelListRequest,
} from "@patchbay/core/types";
import { renderWithI18n } from "../../test/i18n";

const mockInitiateListModels = vi.hoisted(() => vi.fn());
const mockGetListModelsResult = vi.hoisted(() => vi.fn());

vi.mock("@patchbay/core/api", () => ({
  api: {
    initiateListModels: (...args: unknown[]) =>
      mockInitiateListModels(...args),
    getListModelsResult: (...args: unknown[]) =>
      mockGetListModelsResult(...args),
  },
}));

vi.mock("../../common/avatar-upload-control", () => ({
  AvatarUploadControl: () => <div data-testid="avatar-upload" />,
}));

vi.mock("./inspector/runtime-picker", () => ({
  RuntimePicker: () => <div data-testid="runtime-picker" />,
}));

import { AgentDetailInspector } from "./agent-detail-inspector";

const agent = {
  id: "agent-1",
  workspace_id: "workspace-1",
  name: "Lambda",
  description: "Test agent",
  runtime_id: "runtime-1",
} as Agent;

const privateRuntime = {
  id: "runtime-1",
  workspace_id: "workspace-1",
  daemon_id: "daemon-1",
  name: "Private runtime",
  runtime_mode: "local",
  provider: "codex",
  launch_header: "",
  status: "online",
  device_info: "Mac",
  metadata: {},
  owner_id: "user-2",
  visibility: "private",
  last_seen_at: null,
  created_at: "2026-01-01T00:00:00Z",
  updated_at: "2026-01-01T00:00:00Z",
} satisfies AgentRuntime;

const completedModelsRequest = {
  id: "request-1",
  runtime_id: privateRuntime.id,
  status: "completed",
  models: [],
  supported: true,
  created_at: "2026-01-01T00:00:00Z",
  updated_at: "2026-01-01T00:00:00Z",
} satisfies RuntimeModelListRequest;

let queryClient: QueryClient;

function renderInspector(currentUserId: string, overrides: Partial<Agent> = {}) {
  const onUpdate = vi.fn(async () => {});
  renderWithI18n(
    <QueryClientProvider client={queryClient}>
      <AgentDetailInspector
        agent={{ ...agent, ...overrides }}
        runtime={privateRuntime}
        runtimes={[privateRuntime]}
        members={[]}
        currentUserId={currentUserId}
        canEdit
        onUpdate={onUpdate}
      />
    </QueryClientProvider>,
  );
  return { onUpdate };
}

describe("AgentDetailInspector runtime access", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockInitiateListModels.mockResolvedValue(completedModelsRequest);
    mockGetListModelsResult.mockResolvedValue(completedModelsRequest);
    queryClient = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });
  });

  afterEach(() => {
    cleanup();
    queryClient.clear();
  });

  it("does not discover models for another member's private runtime", async () => {
    renderInspector("admin-1");

    await act(async () => {
      await Promise.resolve();
    });

    expect(mockInitiateListModels).not.toHaveBeenCalled();
  });

  it("still discovers models for the private runtime owner", async () => {
    renderInspector(privateRuntime.owner_id);

    await waitFor(() => {
      expect(mockInitiateListModels).toHaveBeenCalledWith(privateRuntime.id, privateRuntime.workspace_id);
    });
  });

  it("keeps the existing effort in the final PATCH for an unknown same-runtime model", async () => {
    const { onUpdate } = renderInspector(privateRuntime.owner_id, {
      model: "gpt-5.6-sol",
      thinking_level: "high",
      service_tier: "",
    });

    fireEvent.click(await screen.findByRole("button", { name: /gpt-5\.6-sol/i }));
    const input = await screen.findByPlaceholderText("Search or type a model ID");
    fireEvent.change(input, { target: { value: "custom-local-build" } });
    fireEvent.click(
      await screen.findByRole("button", { name: 'Use "custom-local-build"' }),
    );

    await waitFor(() =>
      expect(onUpdate).toHaveBeenCalledWith(
        "agent-1",
        expect.objectContaining({
          model: "custom-local-build",
          thinking_level: "high",
        }),
      ),
    );
  });
});
