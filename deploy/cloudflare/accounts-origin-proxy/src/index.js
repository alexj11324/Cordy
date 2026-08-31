// The accounts hostname is the public transport edge for the independently
// released Auth Broker. Identity verification, one-time grants, PKCE
// redemption, and Patchbay sessions remain authoritative in the Rust API.
// This Worker performs no authentication ceremony itself.

const ORIGIN_AUTH_HEADER = "x-patchbay-origin-auth";
const BROKER_AUTH_HEADER = "x-patchbay-desktop-broker-auth";
const AUTH_CONTRACT_HEADER = "x-patchbay-auth-contract-version";
const ATTEMPT_PATH = "/v1/desktop/google/attempt";
const COMPLETE_PATH = "/v1/desktop/google/complete";

function resolveOrigin(env) {
  const raw = String(env.ORIGIN || "").trim();
  let origin;
  try {
    origin = new URL(raw);
  } catch {
    return null;
  }
  if (
    origin.protocol !== "https:" ||
    origin.username ||
    origin.password ||
    origin.search ||
    origin.hash
  ) {
    return null;
  }
  origin.pathname = origin.pathname.replace(/\/+$/, "");
  return origin;
}

function healthResponse() {
  return new Response(JSON.stringify({ status: "ok" }), {
    status: 200,
    headers: {
      "content-type": "application/json; charset=utf-8",
      "cache-control": "no-store",
      "x-patchbay-accounts-proxy": "canonical-origin",
    },
  });
}

function resolveOriginAuthToken(env) {
  const token = String(env.ORIGIN_AUTH_TOKEN || "").trim();
  return /^[a-f0-9]{64}$/.test(token) ? token : null;
}

function authJsonResponse(status, error, extraHeaders = {}) {
  return Response.json(
    { error },
    {
      status,
      headers: {
        "cache-control": "no-store",
        [AUTH_CONTRACT_HEADER]: "1",
        ...extraHeaders,
      },
    },
  );
}

function desktopGoogleRoute(request, pathname) {
  if (pathname === ATTEMPT_PATH) {
    if (request.method !== "POST") {
      return {
        response: authJsonResponse(405, "method_not_allowed", { allow: "POST" }),
      };
    }
    return {
      binding: "DESKTOP_ATTEMPT_RATE_LIMITER",
      keyPrefix: "attempt:v1",
      period: 60,
    };
  }
  if (pathname === COMPLETE_PATH) {
    if (request.method !== "POST") {
      return {
        response: authJsonResponse(405, "method_not_allowed", { allow: "POST" }),
      };
    }
    return {
      binding: "DESKTOP_COMPLETE_RATE_LIMITER",
      keyPrefix: "complete:v1",
      period: 60,
    };
  }

  let decoded = pathname;
  try {
    decoded = decodeURIComponent(pathname);
  } catch {
    return null;
  }
  const normalized = decoded
    .replace(/\/{2,}/g, "/")
    .replace(/\/$/, "")
    .toLowerCase();
  if (normalized === ATTEMPT_PATH || normalized === COMPLETE_PATH) {
    return { response: authJsonResponse(404, "route_not_found") };
  }
  return null;
}

function validClientIp(value) {
  if (
    !value ||
    value.length > 64 ||
    value !== value.trim() ||
    /[\s,\u0000-\u001f\u007f]/.test(value)
  ) {
    return false;
  }
  if (value.includes(":")) {
    try {
      const parsed = new URL(`http://[${value}]/`);
      return parsed.hostname.startsWith("[") && parsed.hostname.endsWith("]");
    } catch {
      return false;
    }
  }
  const parts = value.split(".");
  return (
    parts.length === 4 &&
    parts.every(
      (part) => /^\d{1,3}$/.test(part) && Number(part) >= 0 && Number(part) <= 255,
    )
  );
}

async function enforceDesktopGoogleEdge(request, env, route) {
  const clientIp = request.headers.get("cf-connecting-ip") ?? "";
  const limiter = env[route.binding];
  if (!validClientIp(clientIp) || !limiter || typeof limiter.limit !== "function") {
    return authJsonResponse(503, "auth_edge_unavailable");
  }
  try {
    const result = await limiter.limit({ key: `${route.keyPrefix}:${clientIp.toLowerCase()}` });
    if (!result || result.success !== true) {
      if (result?.success === false) {
        return authJsonResponse(429, "rate_limited", {
          "retry-after": String(route.period),
        });
      }
      return authJsonResponse(503, "auth_edge_unavailable");
    }
  } catch {
    return authJsonResponse(503, "auth_edge_unavailable");
  }
  return null;
}

function sanitizedOriginHeaders(request, incoming, originAuthToken) {
  const headers = new Headers(request.headers);
  for (const name of [...headers.keys()]) {
    const lower = name.toLowerCase();
    if (
      lower === "host" ||
      lower === "forwarded" ||
      lower.startsWith("x-forwarded-") ||
      lower === "x-real-ip" ||
      lower === "true-client-ip" ||
      lower.startsWith("cf-") ||
      lower === ORIGIN_AUTH_HEADER ||
      lower === BROKER_AUTH_HEADER ||
      lower === "x-patchbay-accounts-proxy"
    ) {
      headers.delete(name);
    }
  }
  headers.set("x-forwarded-host", incoming.host);
  headers.set("x-forwarded-proto", "https");
  headers.set("x-patchbay-accounts-proxy", "canonical-origin");
  headers.set(ORIGIN_AUTH_HEADER, originAuthToken);
  return headers;
}

export default {
  async fetch(request, env) {
    const incoming = new URL(request.url);
    if (incoming.pathname === "/health") return healthResponse();

    const desktopGoogle = desktopGoogleRoute(request, incoming.pathname);
    if (desktopGoogle?.response) return desktopGoogle.response;
    const edgeResponse = desktopGoogle
      ? await enforceDesktopGoogleEdge(request, env, desktopGoogle)
      : null;
    if (edgeResponse) return edgeResponse;

    const origin = resolveOrigin(env);
    const originAuthToken = resolveOriginAuthToken(env);
    if (!origin || !originAuthToken) {
      if (desktopGoogle) return authJsonResponse(503, "auth_edge_unavailable");
      return new Response("accounts origin is not configured\n", {
        status: 500,
        headers: { "content-type": "text/plain; charset=utf-8" },
      });
    }

    const target = new URL(origin.href);
    const originPath = origin.pathname === "/" ? "" : origin.pathname;
    target.pathname = `${originPath}${incoming.pathname}`;
    target.search = incoming.search;

    const headers = sanitizedOriginHeaders(request, incoming, originAuthToken);

    const init = {
      method: request.method,
      headers,
      redirect: "manual",
    };
    if (request.method !== "GET" && request.method !== "HEAD") {
      init.body = request.body;
    }

    const response = await fetch(new Request(target, init));
    const responseHeaders = new Headers(response.headers);
    responseHeaders.set("x-patchbay-accounts-proxy", "canonical-origin");
    return new Response(response.body, {
      status: response.status,
      statusText: response.statusText,
      headers: responseHeaders,
    });
  },
};
