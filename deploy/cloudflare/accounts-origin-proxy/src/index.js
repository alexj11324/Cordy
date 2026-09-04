const ORIGIN_AUTH_HEADER = "x-patchbay-origin-auth";
const BROKER_AUTH_HEADER = "x-patchbay-desktop-broker-auth";
const ATTEMPT_PATH = "/v1/desktop/google/attempt";
const COMPLETE_PATH = "/v1/desktop/google/complete";

function response(status, error, extra = {}) { return Response.json({ error }, { status, headers: { "cache-control": "no-store", "x-patchbay-auth-contract-version": "1", ...extra } }); }
function origin(env) { try { const value = new URL(String(env.ORIGIN || "").trim()); if (value.protocol !== "https:" || value.username || value.password || value.search || value.hash) return null; value.pathname = value.pathname.replace(/\/+$/, ""); return value; } catch { return null; } }
function secret(env) { const value = String(env.ORIGIN_AUTH_TOKEN || "").trim(); return /^[a-f0-9]{64}$/.test(value) ? value : null; }
function route(request, path) { if (path !== ATTEMPT_PATH && path !== COMPLETE_PATH) return null; if (request.method !== "POST") return { response: response(405, "method_not_allowed", { allow: "POST" }) }; return { binding: path === ATTEMPT_PATH ? "DESKTOP_ATTEMPT_RATE_LIMITER" : "DESKTOP_COMPLETE_RATE_LIMITER", prefix: path === ATTEMPT_PATH ? "attempt:v1" : "complete:v1", period: 60 }; }
function validIp(value) { if (!value || value.length > 64 || value !== value.trim() || /[\s,\u0000-\u001f\u007f]/.test(value)) return false; if (value.includes(":")) { try { return new URL(`http://[${value}]/`).hostname.startsWith("["); } catch { return false; } } const parts = value.split("."); return parts.length === 4 && parts.every((part) => /^\d{1,3}$/.test(part) && Number(part) <= 255); }
async function limit(request, env, selected) { const ip = request.headers.get("cf-connecting-ip") ?? ""; const limiter = env[selected.binding]; if (!validIp(ip) || !limiter || typeof limiter.limit !== "function") return response(503, "auth_edge_unavailable"); try { const result = await limiter.limit({ key: `${selected.prefix}:${ip.toLowerCase()}` }); if (result?.success === true) return null; return result?.success === false ? response(429, "rate_limited", { "retry-after": String(selected.period) }) : response(503, "auth_edge_unavailable"); } catch { return response(503, "auth_edge_unavailable"); } }
function headers(request, incoming, token) { const clean = new Headers(request.headers); for (const name of [...clean.keys()]) { const lower = name.toLowerCase(); if (lower === "host" || lower === "forwarded" || lower.startsWith("x-forwarded-") || lower === "x-real-ip" || lower === "true-client-ip" || lower.startsWith("cf-") || lower === ORIGIN_AUTH_HEADER || lower === BROKER_AUTH_HEADER || lower === "x-patchbay-accounts-proxy") clean.delete(name); } clean.set("x-forwarded-host", incoming.host); clean.set("x-forwarded-proto", "https"); clean.set("x-patchbay-accounts-proxy", "canonical-origin"); clean.set(ORIGIN_AUTH_HEADER, token); return clean; }

export default { async fetch(request, env) {
  const incoming = new URL(request.url);
  if (incoming.pathname === "/health") return Response.json({ status: "ok" }, { headers: { "cache-control": "no-store", "x-patchbay-accounts-proxy": "canonical-origin" } });
  const selected = route(request, incoming.pathname); if (selected?.response) return selected.response; if (selected) { const limited = await limit(request, env, selected); if (limited) return limited; }
  const targetOrigin = origin(env); const token = secret(env); if (!targetOrigin || !token) return selected ? response(503, "auth_edge_unavailable") : new Response("accounts origin is not configured\n", { status: 500 });
  const target = new URL(targetOrigin); target.pathname = `${targetOrigin.pathname === "/" ? "" : targetOrigin.pathname}${incoming.pathname}`; target.search = incoming.search;
  const init = { method: request.method, headers: headers(request, incoming, token), redirect: "manual" }; if (request.method !== "GET" && request.method !== "HEAD") { init.body = request.body; init.duplex = "half"; }
  const upstream = await fetch(new Request(target, init)); const outgoing = new Headers(upstream.headers); outgoing.set("x-patchbay-accounts-proxy", "canonical-origin"); return new Response(upstream.body, { status: upstream.status, statusText: upstream.statusText, headers: outgoing });
} };
