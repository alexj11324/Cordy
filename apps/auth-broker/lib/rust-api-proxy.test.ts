import { describe, expect, it, vi } from "vitest";
import { AUTH_CONTRACT_HEADER } from "./contract";
import { proxyRustDesktopGoogleRequest } from "./rust-api-proxy";

const brokerOrigin = "https://accounts.aspectlylabs.com";
const apiOrigin = "https://api.aspectlylabs.com";
const rustBrokerAuthToken = "a".repeat(64);
const config = { apiOrigin, brokerOrigin, rustBrokerAuthToken };
const state = "s".repeat(43);
const codeChallenge = "c".repeat(43);
const code = `pbd_${"g".repeat(43)}`;
const binding = { state, code_challenge: codeChallenge };

function request(
  path: string,
  init: {
    body?: unknown;
    authorization?: string;
    contractVersion?: string;
    serviceSecret?: string;
    origin?: string;
  } = {},
) {
  const headers = new Headers({
    "content-type": "application/json",
    origin: init.origin ?? brokerOrigin,
    [AUTH_CONTRACT_HEADER]: init.contractVersion ?? "1",
  });
  if (init.authorization) headers.set("authorization", init.authorization);
  if (init.serviceSecret) {
    headers.set("x-patchbay-desktop-broker-auth", init.serviceSecret);
  }
  return new Request(`${brokerOrigin}${path}`, {
    method: "POST",
    headers,
    body: JSON.stringify(init.body ?? binding),
  });
}

describe("Rust desktop Google proxy", () => {
  it("forwards only the validated attempt binding and contract version", async () => {
    const fetcher = vi
      .fn()
      .mockResolvedValue(
        Response.json({ registered: true, ignored: "not-returned" }),
      );

    const response = await proxyRustDesktopGoogleRequest(
      request("/v1/desktop/google/attempt", {
        serviceSecret: "b".repeat(64),
      }),
      "attempt",
      config,
      fetcher,
    );

    expect(response.status).toBe(200);
    await expect(response.json()).resolves.toEqual({ registered: true });
    expect(response.headers.get(AUTH_CONTRACT_HEADER)).toBe("1");
    const [url, init] = fetcher.mock.calls[0] as [URL, RequestInit];
    expect(url.href).toBe(
      "https://api.aspectlylabs.com/api/desktop-google/attempt",
    );
    expect(init.method).toBe("POST");
    expect(JSON.parse(String(init.body))).toEqual(binding);
    const headers = new Headers(init.headers);
    expect(headers.get("authorization")).toBeNull();
    expect(headers.get(AUTH_CONTRACT_HEADER)).toBe("1");
    expect(headers.get("x-patchbay-desktop-broker-auth")).toBe(
      rustBrokerAuthToken,
    );
  });

  it("uses a Clerk bearer only on complete and returns only the one-time grant", async () => {
    const fetcher = vi.fn().mockResolvedValue(
      Response.json({
        callback_protocol: "patchbay-canary-login-fix-123",
        code,
        token: "must-not-cross-the-broker-boundary",
        user: { id: "must-not-cross-the-broker-boundary" },
      }),
    );

    const response = await proxyRustDesktopGoogleRequest(
      request("/v1/desktop/google/complete", {
        authorization: "Bearer clerk-session-token",
      }),
      "complete",
      config,
      fetcher,
    );

    await expect(response.json()).resolves.toEqual({
      callback_protocol: "patchbay-canary-login-fix-123",
      code,
    });
    const [, init] = fetcher.mock.calls[0] as [URL, RequestInit];
    expect(new Headers(init.headers).get("authorization")).toBe(
      "Bearer clerk-session-token",
    );
    expect(response.headers.get("set-cookie")).toBeNull();
  });

  it("accepts the legacy code-only completion response during rollout", async () => {
    const response = await proxyRustDesktopGoogleRequest(
      request("/v1/desktop/google/complete", {
        authorization: "Bearer clerk-session-token",
      }),
      "complete",
      config,
      vi.fn().mockResolvedValue(Response.json({ code })),
    );

    await expect(response.json()).resolves.toEqual({
      callback_protocol: "patchbay",
      code,
    });
  });

  it("rejects cross-origin, malformed, and unauthenticated completion requests", async () => {
    const fetcher = vi.fn();
    const crossOrigin = await proxyRustDesktopGoogleRequest(
      request("/v1/desktop/google/attempt", {
        origin: "https://attacker.example",
      }),
      "attempt",
      config,
      fetcher,
    );
    const malformed = await proxyRustDesktopGoogleRequest(
      request("/v1/desktop/google/attempt", {
        body: { state: "short", code_challenge: codeChallenge },
      }),
      "attempt",
      config,
      fetcher,
    );
    const noSession = await proxyRustDesktopGoogleRequest(
      request("/v1/desktop/google/complete"),
      "complete",
      config,
      fetcher,
    );
    const wrongVersion = await proxyRustDesktopGoogleRequest(
      request("/v1/desktop/google/attempt", { contractVersion: "2" }),
      "attempt",
      config,
      fetcher,
    );

    expect(crossOrigin.status).toBe(403);
    expect(malformed.status).toBe(400);
    expect(noSession.status).toBe(401);
    expect(wrongVersion.status).toBe(409);
    expect(fetcher).not.toHaveBeenCalled();
  });

  it("does not reflect Rust error bodies or availability details", async () => {
    const rejected = await proxyRustDesktopGoogleRequest(
      request("/v1/desktop/google/complete", {
        authorization: "Bearer clerk-session-token",
      }),
      "complete",
      config,
      vi
        .fn()
        .mockResolvedValue(
          Response.json({ error: "provider-internal-detail" }, { status: 409 }),
        ),
    );
    await expect(rejected.json()).resolves.toEqual({
      error: "authorization_rejected",
    });

    const unavailable = await proxyRustDesktopGoogleRequest(
      request("/v1/desktop/google/attempt"),
      "attempt",
      config,
      vi.fn().mockRejectedValue(new Error("network detail")),
    );
    await expect(unavailable.json()).resolves.toEqual({
      error: "rust_api_unavailable",
    });
  });

  it("fails closed on an oversized upstream response", async () => {
    const response = await proxyRustDesktopGoogleRequest(
      request("/v1/desktop/google/attempt"),
      "attempt",
      config,
      vi
        .fn()
        .mockResolvedValue(
          new Response("x", { headers: { "content-length": "5000" } }),
        ),
    );

    expect(response.status).toBe(502);
    await expect(response.json()).resolves.toEqual({
      error: "invalid_rust_api_response",
    });
  });

  it("stops reading a chunked upstream response as soon as it exceeds the limit", async () => {
    const cancel = vi.fn();
    let pulls = 0;
    const chunkedBody = new ReadableStream<Uint8Array>(
      {
        pull(controller) {
          pulls += 1;
          controller.enqueue(new Uint8Array(4097));
        },
        cancel,
      },
      { highWaterMark: 0 },
    );
    const upstream = new Response(chunkedBody);
    expect(upstream.headers.get("content-length")).toBeNull();

    const response = await proxyRustDesktopGoogleRequest(
      request("/v1/desktop/google/attempt"),
      "attempt",
      config,
      vi.fn().mockResolvedValue(upstream),
    );

    expect(response.status).toBe(502);
    expect(pulls).toBe(1);
    expect(cancel).toHaveBeenCalledOnce();
  });

  it("stops reading a chunked request as soon as it exceeds the limit", async () => {
    const oversized = request("/v1/desktop/google/attempt", {
      body: { state: "s".repeat(5000), code_challenge: codeChallenge },
    });
    expect(oversized.headers.get("content-length")).toBeNull();
    const fetcher = vi.fn();

    const response = await proxyRustDesktopGoogleRequest(
      oversized,
      "attempt",
      config,
      fetcher,
    );

    expect(response.status).toBe(413);
    expect(fetcher).not.toHaveBeenCalled();
  });
});
