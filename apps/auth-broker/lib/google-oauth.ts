export type GoogleSsoParams = {
  strategy: "oauth_google";
  redirectUrl: string;
  redirectCallbackUrl: string;
  oidcPrompt: "select_account";
};

export type GoogleSsoResult = { error: unknown };

export const GOOGLE_OAUTH_START_TIMEOUT_MS = 10_000;

export class GoogleOAuthStartTimeoutError extends Error {
  constructor() {
    super("Google OAuth did not start within the configured deadline");
    this.name = "GoogleOAuthStartTimeoutError";
  }
}

/** Bound only the pre-redirect work; Google user interaction is not timed out. */
export function withGoogleOAuthStartTimeout<T>(
  operation: Promise<T>,
  timeoutMs = GOOGLE_OAUTH_START_TIMEOUT_MS,
): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const timeout = new Promise<never>((_, reject) => {
    timer = setTimeout(
      () => reject(new GoogleOAuthStartTimeoutError()),
      timeoutMs,
    );
  });
  return Promise.race([operation, timeout]).finally(() => {
    if (timer !== undefined) clearTimeout(timer);
  });
}

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

export async function startGoogleOAuth(
  signIn: unknown,
  params: { returnUrl: string; callbackUrl: string; origin: string },
): Promise<GoogleSsoResult> {
  const sso = readGoogleSso(signIn);
  if (!sso) throw new Error("Google sign-in is unavailable");
  return sso({
    strategy: "oauth_google",
    redirectUrl: new URL(params.returnUrl, params.origin).href,
    redirectCallbackUrl: new URL(params.callbackUrl, params.origin).href,
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
