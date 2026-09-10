import { afterEach, describe, expect, it, vi } from "vitest";
import { ApiClient } from "./client";

afterEach(() => vi.unstubAllGlobals());

describe("device authorization API boundary", () => {
  it.each([
    null,
    {},
    { client_name: 42, expires_at: "2026-09-09T22:00:00Z" },
    { client_name: "My CLI", expires_at: "unknown" },
  ])(
    "rejects malformed inspection before the approval screen: %j",
    async (body) => {
      vi.stubGlobal(
        "fetch",
        vi.fn().mockResolvedValue(new Response(JSON.stringify(body))),
      );

      await expect(
        new ApiClient("https://example.test").inspectDeviceAuthorization(
          "ABCD-EFGH",
        ),
      ).rejects.toThrow("Invalid device authorization response");
    },
  );

  it("sends the entered code and parses the requesting client", async () => {
    const body = {
      client_name: "My CLI",
      expires_at: "2026-09-09T22:00:00Z",
    };
    const fetchMock = vi
      .fn()
      .mockResolvedValue(new Response(JSON.stringify(body)));
    vi.stubGlobal("fetch", fetchMock);

    await expect(
      new ApiClient("https://example.test").inspectDeviceAuthorization(
        "ABCD-EFGH",
      ),
    ).resolves.toEqual(body);
    expect(JSON.parse(String(fetchMock.mock.calls[0]?.[1]?.body))).toEqual({
      user_code: "ABCD-EFGH",
    });
  });

  it("sends an explicit denial and accepts the 204 acknowledgement", async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValue(new Response(null, { status: 204 }));
    vi.stubGlobal("fetch", fetchMock);

    await new ApiClient("https://example.test").decideDeviceAuthorization(
      "ABCD-EFGH",
      false,
    );
    expect(JSON.parse(String(fetchMock.mock.calls[0]?.[1]?.body))).toEqual({
      user_code: "ABCD-EFGH",
      approve: false,
    });
  });
});
