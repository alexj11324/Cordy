import { afterEach, describe, expect, it, vi } from "vitest";
import { ApiClient } from "./client";

describe("automation GitHub catalog API", () => {
  afterEach(() => vi.unstubAllGlobals());

  it("uses the automation-writer scoped catalog endpoint", async () => {
    const fetchMock = vi.fn().mockResolvedValue(new Response(JSON.stringify({
      repositories: [{ id: 1, full_name: "acme/private", html_url: "https://github.com/acme/private", clone_url: "https://github.com/acme/private.git", description: null, private: true, archived: false, default_branch: "main" }],
      me_logins: ["octocat"],
    }), { status: 200, headers: { "content-type": "application/json" } }));
    vi.stubGlobal("fetch", fetchMock);

    const client = new ApiClient("https://api.example");
    const catalog = await client.getAutomationGitHubCatalog("automation-1");
    expect(catalog.me_logins).toEqual(["octocat"]);
    expect(fetchMock).toHaveBeenCalledWith(
      "https://api.example/api/automations/automation-1/github-catalog",
      expect.objectContaining({ credentials: "include" }),
    );
  });
});
