import { afterEach, describe, expect, it, vi } from "vitest";
import { ApiClient } from "../api/client";

function jsonResponse(value: unknown, status = 200): Response {
  return new Response(JSON.stringify(value), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("ApiClient automation memory contract", () => {
  it("uses encoded automation memory endpoints and revision-safe writes", async () => {
    const summary = {
      name: "project-notes.md",
      revision: 3,
      updated_at: "2026-09-06T20:00:00Z",
    };
    const file = { ...summary, content: "Keep the release checklist." };
    const updated = { ...file, content: "Keep both checklists.", revision: 4 };
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce(jsonResponse({ items: [summary] }))
      .mockResolvedValueOnce(jsonResponse(file))
      .mockResolvedValueOnce(jsonResponse(updated))
      .mockResolvedValueOnce(new Response(null, { status: 204 }));
    vi.stubGlobal("fetch", fetchMock);

    const client = new ApiClient("https://api.example.test");
    await expect(client.listAutomationMemories("automation/1")).resolves.toEqual({ items: [summary] });
    await expect(client.getAutomationMemory("automation/1", "project-notes.md")).resolves.toEqual(file);
    await expect(client.updateAutomationMemory("automation/1", "project-notes.md", {
      content: "Keep both checklists.",
      expected_revision: 3,
    })).resolves.toEqual(updated);
    await expect(client.deleteAutomationMemory("automation/1", "project-notes.md", 4)).resolves.toBeUndefined();

    expect(fetchMock.mock.calls.map(([url]) => url)).toEqual([
      "https://api.example.test/api/automations/automation%2F1/memories",
      "https://api.example.test/api/automations/automation%2F1/memories/project-notes.md",
      "https://api.example.test/api/automations/automation%2F1/memories/project-notes.md",
      "https://api.example.test/api/automations/automation%2F1/memories/project-notes.md?revision=4",
    ]);
    expect(fetchMock.mock.calls[2]?.[1]).toMatchObject({ method: "PUT" });
    expect(JSON.parse(String(fetchMock.mock.calls[2]?.[1]?.body))).toEqual({
      content: "Keep both checklists.",
      expected_revision: 3,
    });
    expect(fetchMock.mock.calls[3]?.[1]).toMatchObject({ method: "DELETE" });
  });

  it.each([
    ["list", { items: [{ name: "MEMORIES.md", revision: "1", updated_at: "now" }] }],
    ["list", { items: [{ name: "project notes.md", revision: 1, updated_at: "now" }] }],
    ["detail", { name: "MEMORIES.md", revision: 1, updated_at: "now" }],
    ["update", { name: "MEMORIES.md", content: 7, revision: 2, updated_at: "now" }],
  ] as const)("rejects a malformed %s response instead of treating it as empty", async (operation, payload) => {
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue(jsonResponse(payload)));
    const client = new ApiClient("https://api.example.test");

    const request = operation === "list"
      ? client.listAutomationMemories("automation-1")
      : operation === "detail"
        ? client.getAutomationMemory("automation-1", "MEMORIES.md")
        : client.updateAutomationMemory("automation-1", "MEMORIES.md", {
            content: "draft",
            expected_revision: 1,
          });

    await expect(request).rejects.toEqual(expect.objectContaining({
      status: 502,
      message: expect.stringMatching(/invalid automation memory response/i),
    }));
  });

  it("rejects a valid file envelope for a different requested name", async () => {
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue(jsonResponse({
      name: "OTHER.md",
      content: "wrong file",
      revision: 2,
      updated_at: "2026-09-06T20:00:00Z",
    })));

    await expect(
      new ApiClient("https://api.example.test").getAutomationMemory("automation-1", "MEMORIES.md"),
    ).rejects.toMatchObject({ status: 502 });
  });
});
