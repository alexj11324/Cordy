import { afterEach, describe, expect, it, vi } from "vitest";
import { ApiClient } from "./client";
import {
  LinearCatalogResponseSchema,
  LinearDryRunResponseSchema,
  LinearProjectBindingSchema,
} from "./schemas";

describe("Linear API contracts", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("preserves provider values that are newer than this client", () => {
    const catalog = LinearCatalogResponseSchema.parse({
      teams: [],
      projects: [],
      states: [{ id: "state-1", name: "Custom", type: "future_category", color: "#fff" }],
      users: [],
      labels: [],
    });
    expect(catalog.states[0]?.type).toBe("future_category");

    const binding = LinearProjectBindingSchema.parse({
      id: "binding-1",
      workspace_id: "workspace-1",
      connection_id: "connection-1",
      patchbay_project_id: "project-1",
      linear_project_id: "linear-project-1",
      linear_team_id: "team-1",
      status: "provider_added_status",
      sync_mode: "provider_added_mode",
      initial_source_of_truth: null,
      activated_at: null,
      paused_at: null,
      created_by_id: "user-1",
      created_at: "2026-01-01T00:00:00Z",
      updated_at: "2026-01-01T00:00:00Z",
    });
    expect(binding.status).toBe("provider_added_status");
    expect(binding.sync_mode).toBe("provider_added_mode");
  });

  it("loads the catalog through the typed workspace endpoint", async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(
        JSON.stringify({
          teams: [{ id: "team-1", key: "ENG", name: "Engineering" }],
          projects: [],
          states: [],
          users: [],
          labels: [],
        }),
        { status: 200, headers: { "content-type": "application/json" } },
      ),
    );
    vi.stubGlobal("fetch", fetchMock);

    const client = new ApiClient("https://api.example");
    const catalog = await client.getLinearCatalog("workspace-1");

    expect(catalog.teams[0]?.key).toBe("ENG");
    expect(fetchMock).toHaveBeenCalledWith(
      "https://api.example/api/workspaces/workspace-1/linear/catalog",
      expect.objectContaining({ credentials: "include" }),
    );
  });

  it("loads a read-only dry-run preview through the typed workspace endpoint", async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(
        JSON.stringify({
          patchbay_project_id: "project-1",
          linear_project_id: "linear-project-1",
          sync_mode: "import",
          initial_source_of_truth: "linear",
          local_issue_count: 12,
          remote_issue_count: 34,
          remote_issue_count_truncated: false,
          candidate_import_count: 34,
          candidate_publish_count: 0,
          unmapped_remote_status_count: 2,
          exact_link_counts_available: false,
        }),
        { status: 200, headers: { "content-type": "application/json" } },
      ),
    );
    vi.stubGlobal("fetch", fetchMock);

    const client = new ApiClient("https://api.example");
    const preview = await client.dryRunLinearBinding("workspace-1", {
      connection_id: "connection-1",
      patchbay_project_id: "project-1",
      linear_project_id: "linear-project-1",
      linear_team_id: "team-1",
      status: "active",
      sync_mode: "import",
      initial_source_of_truth: "linear",
      status_mapping: {},
      agent_label_mapping: {},
    });

    expect(LinearDryRunResponseSchema.parse(preview).candidate_import_count).toBe(34);
    expect(fetchMock).toHaveBeenCalledWith(
      "https://api.example/api/workspaces/workspace-1/linear/dry-run",
      expect.objectContaining({
        credentials: "include",
        method: "POST",
      }),
    );
  });
});
