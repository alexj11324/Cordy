import {
  AUTH_CONTRACT_HEADER,
  AUTH_CONTRACT_VERSION,
  RUST_ATTEMPT_PATH,
  RUST_COMPLETE_PATH,
  authContractResponseHeaders,
} from "./contract";
import { isDesktopCode, isDesktopHandoffInput } from "./desktop-handoff";

const MAX_REQUEST_BYTES = 4096;
const MAX_SESSION_TOKEN_BYTES = 8192;
const MAX_UPSTREAM_RESPONSE_BYTES = 4096;
const UPSTREAM_TIMEOUT_MS = 10_000;

type ProxyOperation = "attempt" | "complete";
type ProxyConfig = { apiOrigin: string; brokerOrigin: string };
type FetchLike = typeof fetch;

export async function proxyRustDesktopGoogleRequest(
  request: Request,
  operation: ProxyOperation,
  config: ProxyConfig,
  fetcher: FetchLike = fetch,
): Promise<Response> {
  if (request.method !== "POST") return jsonError(405, "method_not_allowed");
  if (request.headers.get("origin") !== config.brokerOrigin) {
    return jsonError(403, "origin_rejected");
  }
  if (
    request.headers.get(AUTH_CONTRACT_HEADER) !== String(AUTH_CONTRACT_VERSION)
  ) {
    return jsonError(409, "contract_version_rejected");
  }
  const contentLengthHeader = request.headers.get("content-length");
  if (contentLengthHeader !== null) {
    const contentLength = Number(contentLengthHeader);
    if (
      !Number.isFinite(contentLength) ||
      contentLength < 0 ||
      contentLength > MAX_REQUEST_BYTES
    ) {
      return jsonError(413, "request_too_large");
    }
  }
  const contentType = request.headers.get("content-type")?.toLowerCase() ?? "";
  if (!contentType.startsWith("application/json")) {
    return jsonError(415, "content_type_rejected");
  }
  const bodyResult = await readBoundedUtf8Body(request.body, MAX_REQUEST_BYTES);
  if (bodyResult.status === "too_large") {
    return jsonError(413, "request_too_large");
  }
  if (bodyResult.status === "invalid") return jsonError(400, "invalid_request");
  let body: unknown;
  try {
    body = JSON.parse(bodyResult.text);
  } catch {
    return jsonError(400, "invalid_request");
  }
  if (!isDesktopHandoffInput(body)) return jsonError(400, "invalid_binding");

  const upstreamHeaders = new Headers({
    accept: "application/json",
    "content-type": "application/json",
    [AUTH_CONTRACT_HEADER]: String(AUTH_CONTRACT_VERSION),
  });
  if (operation === "complete") {
    const authorization = request.headers.get("authorization") ?? "";
    if (
      !authorization.startsWith("Bearer ") ||
      authorization.length <= "Bearer ".length ||
      authorization.length > MAX_SESSION_TOKEN_BYTES ||
      /[\r\n]/.test(authorization)
    ) {
      return jsonError(401, "clerk_session_required");
    }
    upstreamHeaders.set("authorization", authorization);
  }

  const upstreamPath =
    operation === "attempt" ? RUST_ATTEMPT_PATH : RUST_COMPLETE_PATH;
  let upstream: Response;
  try {
    upstream = await fetcher(new URL(upstreamPath, config.apiOrigin), {
      method: "POST",
      headers: upstreamHeaders,
      body: JSON.stringify(body),
      cache: "no-store",
      redirect: "error",
      signal: AbortSignal.timeout(UPSTREAM_TIMEOUT_MS),
    });
  } catch {
    return jsonError(503, "rust_api_unavailable");
  }

  if (!upstream.ok) {
    const status = upstream.status >= 400 && upstream.status < 500 ? upstream.status : 502;
    return jsonError(status, "authorization_rejected");
  }
  const upstreamLengthHeader = upstream.headers.get("content-length");
  if (upstreamLengthHeader !== null) {
    const upstreamLength = Number(upstreamLengthHeader);
    if (
      !Number.isFinite(upstreamLength) ||
      upstreamLength < 0 ||
      upstreamLength > MAX_UPSTREAM_RESPONSE_BYTES
    ) {
      return jsonError(502, "invalid_rust_api_response");
    }
  }
  const upstreamBody = await readBoundedUtf8Body(
    upstream.body,
    MAX_UPSTREAM_RESPONSE_BYTES,
  );
  if (upstreamBody.status !== "ok") {
    return jsonError(502, "invalid_rust_api_response");
  }
  let payload: unknown;
  try {
    payload = JSON.parse(upstreamBody.text);
  } catch {
    return jsonError(502, "invalid_rust_api_response");
  }
  if (!payload || typeof payload !== "object" || Array.isArray(payload)) {
    return jsonError(502, "invalid_rust_api_response");
  }
  const record = payload as Record<string, unknown>;
  if (operation === "attempt" && record.registered === true) {
    return Response.json(
      { registered: true },
      { headers: authContractResponseHeaders() },
    );
  }
  if (operation === "complete" && isDesktopCode(record.code)) {
    return Response.json(
      { code: record.code },
      { headers: authContractResponseHeaders() },
    );
  }
  return jsonError(502, "invalid_rust_api_response");
}

async function readBoundedUtf8Body(
  body: ReadableStream<Uint8Array> | null,
  limit: number,
): Promise<
  | { status: "ok"; text: string }
  | { status: "invalid" }
  | { status: "too_large" }
> {
  if (!body) return { status: "ok", text: "" };
  const reader = body.getReader();
  const chunks: Uint8Array[] = [];
  let byteLength = 0;
  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      byteLength += value.byteLength;
      if (byteLength > limit) {
        await reader.cancel().catch(() => undefined);
        return { status: "too_large" };
      }
      chunks.push(value);
    }
  } catch {
    return { status: "invalid" };
  } finally {
    reader.releaseLock();
  }

  const bytes = new Uint8Array(byteLength);
  let offset = 0;
  for (const chunk of chunks) {
    bytes.set(chunk, offset);
    offset += chunk.byteLength;
  }
  try {
    return {
      status: "ok",
      text: new TextDecoder("utf-8", { fatal: true }).decode(bytes),
    };
  } catch {
    return { status: "invalid" };
  }
}

function jsonError(status: number, code: string): Response {
  return Response.json(
    { error: code },
    { status, headers: authContractResponseHeaders() },
  );
}
