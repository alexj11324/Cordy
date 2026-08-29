import { createClerkClient, verifyToken, type User } from "@clerk/backend";

const PUBLIC_ORIGIN = "https://accounts.aspectlylabs.com";
const CALLBACK_PATH = "/oauth/google/callback";
const COMPLETE_PATH = "/oauth/google/complete";
const SIGN_IN_PATH = "/oauth/google/sign-in";
const SIGN_UP_PATH = "/oauth/google/sign-up";
const HEALTH_PATH = "/oauth/google/healthz";
const READY_PATH = "/oauth/google/readyz";
const DEEP_LINK_ORIGIN = "patchbay://auth/callback";
const CLERK_SCRIPT_URL =
  "https://clerk.aspectlylabs.com/npm/@clerk/clerk-js@6/dist/clerk.browser.js";
const CODE_TTL_MS = 60_000;
const MAX_CODE_LENGTH = 128;

type PendingProfile = {
  clerkUserId: string;
  email: string;
  name: string;
  picture: string;
  expiresAt: number;
};

interface DurableObjectStorageLike {
  get<T>(key: string): Promise<T | undefined>;
  put<T>(key: string, value: T): Promise<void>;
  delete(key: string): Promise<boolean>;
}

interface DurableObjectStateLike {
  storage: DurableObjectStorageLike;
  blockConcurrencyWhile<T>(callback: () => Promise<T>): Promise<T>;
}

interface DurableObjectStubLike {
  fetch(input: RequestInfo | URL, init?: RequestInit): Promise<Response>;
}

interface DurableObjectNamespaceLike {
  idFromName(name: string): unknown;
  get(id: unknown): DurableObjectStubLike;
}

export interface AuthBrokerEnv {
  CLERK_PUBLISHABLE_KEY?: string;
  CLERK_SECRET_KEY?: string;
  BROKER_SHARED_SECRET?: string;
  AUTH_CODE_STORE?: DurableObjectNamespaceLike;
}

/**
 * Durable Object storage for an authorization code. The consume operation is
 * wrapped in blockConcurrencyWhile so a concurrent second exchange cannot
 * observe the record between get and delete.
 */
export class AuthCodeStore {
  constructor(private readonly state: DurableObjectStateLike) {}

  async fetch(request: Request): Promise<Response> {
    const path = new URL(request.url).pathname;
    if (request.method === "PUT" && path === "/put") {
      return this.put(request);
    }
    if (request.method === "POST" && path === "/consume") {
      return this.consume();
    }
    return new Response("Not found", { status: 404 });
  }

  private async put(request: Request): Promise<Response> {
    let record: PendingProfile;
    try {
      record = (await request.json()) as PendingProfile;
    } catch {
      return new Response("Invalid record", { status: 400 });
    }
    if (!isPendingProfile(record)) {
      return new Response("Invalid record", { status: 400 });
    }

    return this.state.blockConcurrencyWhile(async () => {
      await this.state.storage.put("pending", record);
      return new Response(null, { status: 204 });
    });
  }

  private async consume(): Promise<Response> {
    return this.state.blockConcurrencyWhile(async () => {
      const record = await this.state.storage.get<PendingProfile>("pending");
      if (!record) return json({ error: "invalid_or_expired_code" }, 410);

      // Delete before returning the profile. Even an expired record is
      // consumed, so repeated attempts cannot become a timing oracle.
      await this.state.storage.delete("pending");
      if (record.expiresAt <= Date.now()) {
        return json({ error: "invalid_or_expired_code" }, 410);
      }
      return json(record);
    });
  }
}

export function desktopDeepLink(code: string): string {
  return `${DEEP_LINK_ORIGIN}?code=${encodeURIComponent(code)}`;
}

export function isDesktopPlatform(value: string | null): boolean {
  return value === "desktop";
}

export default {
  async fetch(
    request: Request,
    env: AuthBrokerEnv,
  ): Promise<Response> {
    return route(request, env);
  },
};

async function route(
  request: Request,
  env: AuthBrokerEnv,
): Promise<Response> {
  const url = new URL(request.url);

  if (
    request.method === "GET" &&
    (url.pathname === HEALTH_PATH || url.pathname === "/healthz")
  ) {
    return json({ service: "auth-broker", status: "ok" });
  }
  if (
    request.method === "GET" &&
    (url.pathname === READY_PATH || url.pathname === "/readyz")
  ) {
    return readiness(env);
  }

  if (request.method === "GET" && url.pathname === "/oauth/google") {
    if (!isDesktopPlatform(url.searchParams.get("platform"))) {
      return json({ error: "desktop_platform_required" }, 404);
    }
    if (!env.CLERK_PUBLISHABLE_KEY) {
      return serviceUnavailable();
    }
    return htmlPage(startPageScript(env.CLERK_PUBLISHABLE_KEY));
  }

  if (request.method === "GET" && url.pathname === CALLBACK_PATH) {
    if (!env.CLERK_PUBLISHABLE_KEY) return serviceUnavailable();
    return htmlPage(callbackPageScript(env.CLERK_PUBLISHABLE_KEY));
  }

  if (request.method === "GET" && url.pathname === COMPLETE_PATH) {
    if (!isDesktopPlatform(url.searchParams.get("platform"))) {
      return json({ error: "desktop_platform_required" }, 404);
    }
    if (!env.CLERK_PUBLISHABLE_KEY) return serviceUnavailable();
    return htmlPage(completePageScript(env.CLERK_PUBLISHABLE_KEY));
  }

  if (
    request.method === "GET" &&
    (url.pathname === SIGN_IN_PATH || url.pathname === SIGN_UP_PATH)
  ) {
    if (!isDesktopPlatform(url.searchParams.get("platform"))) {
      return json({ error: "desktop_platform_required" }, 404);
    }
    if (!env.CLERK_PUBLISHABLE_KEY) return serviceUnavailable();
    return htmlPage(
      url.pathname === SIGN_IN_PATH
        ? signInPageScript(env.CLERK_PUBLISHABLE_KEY)
        : signUpPageScript(env.CLERK_PUBLISHABLE_KEY),
    );
  }

  if (request.method === "POST" && url.pathname === COMPLETE_PATH) {
    return complete(request, env);
  }

  if (request.method === "POST" && url.pathname === "/oauth/google/exchange") {
    return exchange(request, env);
  }

  if (request.method === "OPTIONS") {
    return corsPreflight(request);
  }

  return new Response("Not found", { status: 404 });
}

async function complete(
  request: Request,
  env: AuthBrokerEnv,
): Promise<Response> {
  if (!hasExactOrigin(request)) {
    return json({ error: "origin_not_allowed" }, 403);
  }
  if (!env.CLERK_SECRET_KEY || !env.AUTH_CODE_STORE) {
    return serviceUnavailable();
  }

  const sessionToken = bearerToken(request.headers.get("Authorization"));
  if (!sessionToken) return json({ error: "authorization_required" }, 401);

  let clerkUserId: string;
  try {
    const verified = await verifyToken(sessionToken, {
      secretKey: env.CLERK_SECRET_KEY,
      authorizedParties: [PUBLIC_ORIGIN],
    });
    if (typeof verified.sub !== "string" || verified.sub.length === 0) {
      return json({ error: "invalid_session" }, 401);
    }
    clerkUserId = verified.sub;
  } catch {
    return json({ error: "invalid_session" }, 401);
  }

  let clerkUser: User;
  try {
    const clerk = createClerkClient({ secretKey: env.CLERK_SECRET_KEY });
    clerkUser = await clerk.users.getUser(clerkUserId);
  } catch {
    return json({ error: "identity_lookup_failed" }, 502);
  }

  const primaryEmail = clerkUser.primaryEmailAddress;
  const email = primaryEmail?.emailAddress?.trim().toLowerCase();
  if (!email || primaryEmail.verification?.status !== "verified") {
    return json({ error: "verified_email_required" }, 422);
  }

  const code = randomCode();
  const record: PendingProfile = {
    clerkUserId,
    email,
    name: displayName(clerkUser.firstName, clerkUser.lastName, email),
    picture: typeof clerkUser.imageUrl === "string" ? clerkUser.imageUrl : "",
    expiresAt: Date.now() + CODE_TTL_MS,
  };
  const id = env.AUTH_CODE_STORE.idFromName(code);
  const stored = await env.AUTH_CODE_STORE.get(id).fetch("https://auth-code/put", {
    method: "PUT",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(record),
  });
  if (!stored.ok) return serviceUnavailable();

  return json({ code, expires_in: CODE_TTL_MS / 1000 });
}

async function exchange(
  request: Request,
  env: AuthBrokerEnv,
): Promise<Response> {
  if (!env.BROKER_SHARED_SECRET || !env.AUTH_CODE_STORE) {
    return serviceUnavailable();
  }
  if (
    !constantTimeEqual(
      request.headers.get("X-Patchbay-Broker-Secret") ?? "",
      env.BROKER_SHARED_SECRET,
    )
  ) {
    return json({ error: "not_allowed" }, 401);
  }

  let body: unknown;
  try {
    body = await request.json();
  } catch {
    return json({ error: "invalid_request" }, 400);
  }
  const code =
    body && typeof body === "object" && typeof (body as { code?: unknown }).code === "string"
      ? (body as { code: string }).code
      : "";
  if (!isOpaqueCode(code)) return json({ error: "invalid_or_expired_code" }, 400);

  const id = env.AUTH_CODE_STORE.idFromName(code);
  const consumed = await env.AUTH_CODE_STORE.get(id).fetch("https://auth-code/consume", {
    method: "POST",
  });
  if (!consumed.ok) return json({ error: "invalid_or_expired_code" }, 400);

  const profile = (await consumed.json()) as PendingProfile;
  return json({
    clerk_user_id: profile.clerkUserId,
    email: profile.email,
    name: profile.name,
    picture: profile.picture,
  });
}

function readiness(env: AuthBrokerEnv): Response {
  const ready = Boolean(
    env.CLERK_PUBLISHABLE_KEY &&
      env.CLERK_SECRET_KEY &&
      env.BROKER_SHARED_SECRET &&
      env.AUTH_CODE_STORE,
  );
  return json({ service: "auth-broker", status: ready ? "ready" : "not_ready" }, ready ? 200 : 503);
}

function hasExactOrigin(request: Request): boolean {
  return request.headers.get("Origin") === PUBLIC_ORIGIN;
}

function bearerToken(value: string | null): string | null {
  if (!value?.startsWith("Bearer ")) return null;
  const token = value.slice("Bearer ".length).trim();
  return token.length > 0 && !token.includes(" ") ? token : null;
}

function isOpaqueCode(value: string): boolean {
  return (
    value.length > 0 &&
    value.length <= MAX_CODE_LENGTH &&
    /^[A-Za-z0-9_-]+$/.test(value)
  );
}

function randomCode(): string {
  const bytes = new Uint8Array(32);
  crypto.getRandomValues(bytes);
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary).replaceAll("+", "-").replaceAll("/", "_").replaceAll("=", "");
}

function displayName(
  firstName: string | null,
  lastName: string | null,
  email: string,
): string {
  const name = [firstName, lastName]
    .filter((value): value is string => typeof value === "string" && value.trim().length > 0)
    .join(" ")
    .trim();
  return name || email.split("@", 1)[0] || email;
}

function isPendingProfile(value: unknown): value is PendingProfile {
  if (!value || typeof value !== "object") return false;
  const record = value as Record<string, unknown>;
  return (
    typeof record.clerkUserId === "string" && record.clerkUserId.length > 0 &&
    typeof record.email === "string" && record.email.length > 0 &&
    typeof record.name === "string" && record.name.length > 0 &&
    typeof record.picture === "string" &&
    typeof record.expiresAt === "number" && Number.isFinite(record.expiresAt)
  );
}

function constantTimeEqual(left: string, right: string): boolean {
  const leftBytes = new TextEncoder().encode(left);
  const rightBytes = new TextEncoder().encode(right);
  let difference = leftBytes.length ^ rightBytes.length;
  const length = Math.max(leftBytes.length, rightBytes.length);
  for (let index = 0; index < length; index += 1) {
    difference |= (leftBytes[index] ?? 0) ^ (rightBytes[index] ?? 0);
  }
  return difference === 0;
}

function startPageScript(publishableKey: string): string {
  return `
    (async () => {
      const status = document.getElementById("status");
      try {
        const clerk = new window.Clerk(${JSON.stringify(publishableKey)});
        await clerk.load();
        await clerk.client.signIn.authenticateWithRedirect({
          strategy: "oauth_google",
          redirectUrl: new URL(${JSON.stringify(CALLBACK_PATH)}, location.origin).toString(),
          redirectUrlComplete: new URL(${JSON.stringify(`${COMPLETE_PATH}?platform=desktop`)}, location.origin).toString(),
          continueSignIn: true,
          continueSignUp: true,
        });
      } catch {
        if (status) status.textContent = "Google sign-in could not be started.";
      }
    })();
  `;
}

function callbackPageScript(publishableKey: string): string {
  return `
    (async () => {
      const status = document.getElementById("status");
      try {
        const clerk = new window.Clerk(${JSON.stringify(publishableKey)});
        await clerk.load();
        const completeUrl = new URL(
          ${JSON.stringify(`${COMPLETE_PATH}?platform=desktop`)},
          location.origin,
        ).toString();
        const signInUrl = new URL(
          ${JSON.stringify(`${SIGN_IN_PATH}?platform=desktop`)},
          location.origin,
        ).toString();
        const signUpUrl = new URL(
          ${JSON.stringify(`${SIGN_UP_PATH}?platform=desktop`)},
          location.origin,
        ).toString();
        await clerk.handleRedirectCallback(
          {
            signInUrl,
            signUpUrl,
            firstFactorUrl: signInUrl,
            secondFactorUrl: signInUrl,
            resetPasswordUrl: signInUrl,
            continueSignUpUrl: signUpUrl,
            verifyEmailAddressUrl: signUpUrl,
            verifyPhoneNumberUrl: signUpUrl,
            signInFallbackRedirectUrl: completeUrl,
            signUpFallbackRedirectUrl: completeUrl,
          },
          async (to) => {
            const target = new URL(to, location.origin);
            const allowedPaths = new Set([
              ${JSON.stringify(CALLBACK_PATH)},
              ${JSON.stringify(COMPLETE_PATH)},
              ${JSON.stringify(SIGN_IN_PATH)},
              ${JSON.stringify(SIGN_UP_PATH)},
            ]);
            if (target.origin !== location.origin || !allowedPaths.has(target.pathname)) {
              throw new Error("Clerk returned an unexpected broker redirect");
            }
            location.replace(target.toString());
          },
        );
        location.replace(completeUrl);
      } catch {
        if (status) status.textContent = "Google sign-in could not be completed.";
      }
    })();
  `;
}

function signInPageScript(publishableKey: string): string {
  return `
    (async () => {
      const status = document.getElementById("status");
      try {
        const clerk = new window.Clerk(${JSON.stringify(publishableKey)});
        await clerk.load();
        clerk.mountSignIn(status, {
          routing: "path",
          path: ${JSON.stringify(SIGN_IN_PATH)},
          signUpUrl: new URL(
            ${JSON.stringify(`${SIGN_UP_PATH}?platform=desktop`)},
            location.origin,
          ).toString(),
          fallbackRedirectUrl: new URL(
            ${JSON.stringify(`${COMPLETE_PATH}?platform=desktop`)},
            location.origin,
          ).toString(),
        });
      } catch {
        if (status) status.textContent = "Additional sign-in verification could not be loaded.";
      }
    })();
  `;
}

function signUpPageScript(publishableKey: string): string {
  return `
    (async () => {
      const status = document.getElementById("status");
      try {
        const clerk = new window.Clerk(${JSON.stringify(publishableKey)});
        await clerk.load();
        clerk.mountSignUp(status, {
          routing: "path",
          path: ${JSON.stringify(SIGN_UP_PATH)},
          signInUrl: new URL(
            ${JSON.stringify(`${SIGN_IN_PATH}?platform=desktop`)},
            location.origin,
          ).toString(),
          fallbackRedirectUrl: new URL(
            ${JSON.stringify(`${COMPLETE_PATH}?platform=desktop`)},
            location.origin,
          ).toString(),
        });
      } catch {
        if (status) status.textContent = "Additional sign-up verification could not be loaded.";
      }
    })();
  `;
}

function completePageScript(publishableKey: string): string {
  return `
    (async () => {
      const status = document.getElementById("status");
      try {
        const clerk = new window.Clerk(${JSON.stringify(publishableKey)});
        await clerk.load();
        const completeUrl = new URL(
          ${JSON.stringify(`${COMPLETE_PATH}?platform=desktop`)},
          location.origin,
        ).toString();
        const task = clerk.session?.currentTask;
        if (task) {
          if (!(status instanceof HTMLDivElement)) throw new Error("missing task mount");
          const taskProps = { redirectUrlComplete: completeUrl };
          if (task.key === "choose-organization") {
            clerk.mountTaskChooseOrganization(status, taskProps);
          } else if (task.key === "reset-password") {
            clerk.mountTaskResetPassword(status, taskProps);
          } else if (task.key === "setup-mfa") {
            clerk.mountTaskSetupMFA(status, taskProps);
          } else {
            throw new Error("unsupported session task");
          }
          return;
        }
        const sessionToken = await clerk.session?.getToken();
        if (!sessionToken) throw new Error("missing session");
        const response = await fetch(new URL(${JSON.stringify(COMPLETE_PATH)}, location.origin), {
          method: "POST",
          headers: { Authorization: "Bearer " + sessionToken },
          credentials: "same-origin",
          cache: "no-store",
        });
        if (!response.ok) throw new Error("broker exchange failed");
        const result = await response.json();
        if (typeof result.code !== "string") throw new Error("missing authorization code");
        location.replace(${JSON.stringify(DEEP_LINK_ORIGIN)} + "?code=" + encodeURIComponent(result.code));
      } catch {
        if (status) status.textContent = "Google sign-in needs to be completed again.";
      }
    })();
  `;
}

function htmlPage(script: string): Response {
  const nonce = randomCode();
  const body = `<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width,initial-scale=1">
    <title>Patchbay sign-in</title>
    <style nonce="${nonce}">body{font-family:system-ui,sans-serif;display:grid;min-height:100vh;place-items:center;margin:0;color:#20242a}main{max-width:32rem;padding:2rem;text-align:center}p{color:#5b6470}</style>
  </head>
  <body><main><h1>Patchbay</h1><div id="status">Continuing with Google…</div></main>
    <script nonce="${nonce}" src="${CLERK_SCRIPT_URL}"></script>
    <script nonce="${nonce}">${script}</script>
  </body>
</html>`;
  return new Response(body, {
    headers: {
      "Content-Type": "text/html; charset=utf-8",
      "Cache-Control": "no-store",
      "Referrer-Policy": "no-referrer",
      "X-Content-Type-Options": "nosniff",
      "Content-Security-Policy": [
        "default-src 'none'",
        `script-src 'self' 'nonce-${nonce}' https://clerk.aspectlylabs.com`,
        // Clerk's mounted task UI injects style attributes and style tags that
        // cannot inherit this bootstrap nonce. Keep scripts nonce-bound while
        // allowing only the UI's inline styles; no script/connect/frame
        // wildcard is permitted.
        "style-src 'self' 'unsafe-inline' https://clerk.aspectlylabs.com",
        "connect-src 'self' https://clerk.aspectlylabs.com",
        "img-src 'self' data: https://clerk.aspectlylabs.com",
        "frame-src 'self' https://clerk.aspectlylabs.com",
        "form-action 'self'",
        "base-uri 'none'",
        "object-src 'none'",
        "frame-ancestors 'none'",
      ].join("; "),
    },
  });
}

function corsPreflight(request: Request): Response {
  if (request.headers.get("Origin") !== PUBLIC_ORIGIN) {
    return new Response(null, { status: 403 });
  }
  return new Response(null, {
    status: 204,
    headers: {
      "Access-Control-Allow-Origin": PUBLIC_ORIGIN,
      "Access-Control-Allow-Methods": "POST, OPTIONS",
      "Access-Control-Allow-Headers": "Authorization, Content-Type",
      Vary: "Origin",
    },
  });
}

function serviceUnavailable(): Response {
  return json({ error: "auth_broker_not_configured" }, 503);
}

function json(value: unknown, status = 200): Response {
  return new Response(JSON.stringify(value), {
    status,
    headers: {
      "Content-Type": "application/json; charset=utf-8",
      "Cache-Control": "no-store",
      "X-Content-Type-Options": "nosniff",
    },
  });
}
