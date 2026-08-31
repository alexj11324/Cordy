// The accounts hostname is the public transport edge for the independently
// released Auth Broker. Identity verification, one-time grants, PKCE
// redemption, and Patchbay sessions remain authoritative in the Rust API.
// This Worker performs no authentication ceremony itself.

const ORIGIN_AUTH_HEADER = "x-patchbay-origin-auth";

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

export default {
  async fetch(request, env) {
    const incoming = new URL(request.url);
    if (incoming.pathname === "/health") return healthResponse();

    const origin = resolveOrigin(env);
    const originAuthToken = resolveOriginAuthToken(env);
    if (!origin || !originAuthToken) {
      return new Response("accounts origin is not configured\n", {
        status: 500,
        headers: { "content-type": "text/plain; charset=utf-8" },
      });
    }

    const target = new URL(origin.href);
    target.pathname = `${origin.pathname}${incoming.pathname}`.replace(
      /\/+/g,
      "/",
    );
    target.search = incoming.search;

    const headers = new Headers(request.headers);
    headers.delete("host");
    headers.set("x-forwarded-host", incoming.host);
    headers.set("x-forwarded-proto", "https");
    headers.set("x-patchbay-accounts-proxy", "canonical-origin");
    headers.set(ORIGIN_AUTH_HEADER, originAuthToken);

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
