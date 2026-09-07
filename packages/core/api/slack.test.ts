import { afterEach, describe, expect, it, vi } from "vitest";
import { ApiClient } from "./client";

describe("Slack automation catalog API", () => {
  afterEach(() => vi.unstubAllGlobals());

  it("loads provider channel identities from the workspace endpoint", async () => {
    const fetchMock = vi.fn().mockResolvedValue(new Response(JSON.stringify({
      channels: [{ installation_id: "installation-1", team_id: "T1", id: "C1", name: "incidents" }],
    }), { status: 200, headers: { "content-type": "application/json" } }));
    vi.stubGlobal("fetch", fetchMock);

    const client = new ApiClient("https://api.example");
    await expect(client.getSlackAutomationCatalog("workspace-1")).resolves.toEqual({
      channels: [{ installation_id: "installation-1", team_id: "T1", id: "C1", name: "incidents" }],
    });
    expect(fetchMock).toHaveBeenCalledWith(
      "https://api.example/api/workspaces/workspace-1/slack/automation-catalog",
      expect.objectContaining({ credentials: "include" }),
    );
  });
});
