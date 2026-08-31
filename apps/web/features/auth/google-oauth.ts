import { buildBrokerRoute } from "./broker-path";

export type GoogleSsoParams = {
  strategy: "oauth_google";
  redirectUrl: string;
  redirectCallbackUrl: string;
  oidcPrompt: "select_account";
};

export type GoogleSsoResult = { error: unknown };

type GoogleOAuthAttempt = {
  status?: string | null;
  isTransferable?: boolean;
  existingSession?: { sessionId?: string } | null;
};

type SearchLike = { has: (name: string) => boolean };

function asRecord(value: unknown): Record<string, unknown> | null {
  if (typeof value !== "object" || value === null) return null;
  return value as Record<string, unknown>;
}

/** Clerk FAPI allowlists absolute URLs; relative paths stay relative in production. */
export function toSameOriginUrl(pathOrUrl: string, origin: string): string {
  return new URL(pathOrUrl, origin).href;
}

/** Move a Clerk ticket/nonce off the start route without dropping the desktop binding. */
export function googleOAuthCallbackHref(input: {
  pathname: string;
  search: string;
  hash: string;
}): string {
  const path = buildBrokerRoute(
    input.pathname,
    "/oauth/google",
    "/oauth/google/callback",
  );
  return `${path}${input.search}${input.hash}`;
}

/** Clerk returns here with a ticket/nonce instead of a finished SignIn resource. */
export function hasClerkOAuthReturn(
  searchParams: SearchLike,
  hash = "",
): boolean {
  return (
    searchParams.has("rotating_token_nonce") ||
    searchParams.has("__clerk_status") ||
    searchParams.has("__clerk_ticket") ||
    /__clerk/i.test(hash)
  );
}

export function readGoogleSso(
  signIn: unknown,
): ((params: GoogleSsoParams) => Promise<GoogleSsoResult>) | null {
  const record = asRecord(signIn);
  if (!record) return null;
  if (typeof record.sso === "function") {
    return record.sso.bind(record) as (
      params: GoogleSsoParams,
    ) => Promise<GoogleSsoResult>;
  }
  const future = asRecord(record.__internal_future);
  if (future && typeof future.sso === "function") {
    return future.sso.bind(future) as (
      params: GoogleSsoParams,
    ) => Promise<GoogleSsoResult>;
  }
  return null;
}

export function canStartGoogleOAuth(signIn: unknown): boolean {
  return readGoogleSso(signIn) !== null;
}

/** Start Google SSO through the Core 3 flow consumed by our callback page. */
export async function startGoogleOAuth(
  signIn: unknown,
  params: { returnUrl: string; callbackUrl: string; origin: string },
): Promise<GoogleSsoResult> {
  const returnUrl = toSameOriginUrl(params.returnUrl, params.origin);
  const callbackUrl = toSameOriginUrl(params.callbackUrl, params.origin);
  const sso = readGoogleSso(signIn);
  if (!sso) throw new Error("Google sign-in is unavailable");
  return sso({
    strategy: "oauth_google",
    redirectUrl: returnUrl,
    redirectCallbackUrl: callbackUrl,
    oidcPrompt: "select_account",
  });
}

export function googleOAuthAttemptIsReady(
  signIn: GoogleOAuthAttempt | null | undefined,
  signUp: GoogleOAuthAttempt | null | undefined,
): boolean {
  if (!signIn || !signUp) return false;
  return (
    signIn.status === "complete" ||
    signUp.status === "complete" ||
    signIn.isTransferable === true ||
    signUp.isTransferable === true ||
    Boolean(signIn.existingSession?.sessionId) ||
    Boolean(signUp.existingSession?.sessionId) ||
    // Any hydrated status (including MFA / missing fields) is ready for
    // complete() to accept or fail closed. Only null/empty means "still loading".
    (signIn.status != null && signIn.status !== "")
  );
}

export async function consumeGoogleOAuthNonce(
  signIn: unknown,
  rotatingTokenNonce: string | null,
): Promise<boolean> {
  if (!rotatingTokenNonce) return true;
  const record = asRecord(signIn);
  if (!record) return false;
  const future = asRecord(record.__internal_future);
  const reload =
    (typeof record.reload === "function" ? record.reload.bind(record) : null) ??
    (future && typeof future.reload === "function"
      ? future.reload.bind(future)
      : null);
  if (typeof reload !== "function") return false;
  await (
    reload as (params: { rotatingTokenNonce: string }) => Promise<unknown>
  )({ rotatingTokenNonce });
  return true;
}
