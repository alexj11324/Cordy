import { AUTH_CONTRACT_HEADER, AUTH_CONTRACT_VERSION, GO_ATTEMPT_PATH, GO_COMPLETE_PATH, authContractResponseHeaders } from "./contract";
import { isDesktopCallbackProtocol, isDesktopCode, isDesktopHandoffInput } from "./desktop-handoff";
const BROKER_AUTH_HEADER = "x-patchbay-desktop-broker-auth";
type Config = { apiOrigin: string; brokerOrigin: string; goBrokerAuthToken: string };
export async function proxyGoDesktopGoogleRequest(request: Request, operation: "attempt" | "complete", config: Config, fetcher: typeof fetch = fetch): Promise<Response> {
  if (request.method !== "POST") return failure(405, "method_not_allowed");
  if (request.headers.get("origin") !== config.brokerOrigin) return failure(403, "origin_rejected");
  if (request.headers.get(AUTH_CONTRACT_HEADER) !== String(AUTH_CONTRACT_VERSION)) return failure(409, "contract_version_rejected");
  if (!(request.headers.get("content-type") ?? "").toLowerCase().startsWith("application/json")) return failure(415, "content_type_rejected");
  const declared = Number(request.headers.get("content-length") ?? 0); if (declared > 4096) return failure(413, "request_too_large");
  const bytes = new Uint8Array(await request.arrayBuffer()); if (bytes.byteLength > 4096) return failure(413, "request_too_large");
  let body: unknown; try { body = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(bytes)); } catch { return failure(400, "invalid_request"); }
  if (!isDesktopHandoffInput(body)) return failure(400, "invalid_binding");
  const headers = new Headers({ accept: "application/json", "content-type": "application/json", [AUTH_CONTRACT_HEADER]: String(AUTH_CONTRACT_VERSION), [BROKER_AUTH_HEADER]: config.goBrokerAuthToken });
  if (operation === "complete") { const authorization = request.headers.get("authorization") ?? ""; if (!authorization.startsWith("Bearer ") || authorization.length > 8192 || /[\r\n]/.test(authorization)) return failure(401, "clerk_session_required"); headers.set("authorization", authorization); }
  let upstream: Response; try { upstream = await fetcher(new URL(operation === "attempt" ? GO_ATTEMPT_PATH : GO_COMPLETE_PATH, config.apiOrigin), { method: "POST", headers, body: JSON.stringify(body), redirect: "error", cache: "no-store", signal: AbortSignal.timeout(10_000) }); } catch { return failure(503, "go_api_unavailable"); }
  if (!upstream.ok) return failure(upstream.status >= 400 && upstream.status < 500 ? upstream.status : 502, "authorization_rejected");
  const responseBytes = new Uint8Array(await upstream.arrayBuffer()); if (responseBytes.byteLength > 4096) return failure(502, "invalid_go_api_response");
  let payload: unknown; try { payload = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(responseBytes)); } catch { return failure(502, "invalid_go_api_response"); }
  const record = payload && typeof payload === "object" && !Array.isArray(payload) ? payload as Record<string, unknown> : {};
  if (operation === "attempt" && record.registered === true) return Response.json({ registered: true }, { headers: authContractResponseHeaders() });
  if (operation === "complete" && isDesktopCode(record.code) && isDesktopCallbackProtocol(record.callback_protocol)) return Response.json({ callback_protocol: record.callback_protocol, code: record.code }, { headers: authContractResponseHeaders() });
  return failure(502, "invalid_go_api_response");
}
function failure(status: number, error: string): Response { return Response.json({ error }, { status, headers: authContractResponseHeaders() }); }
