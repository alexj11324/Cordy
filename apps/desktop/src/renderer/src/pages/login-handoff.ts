import { buildDesktopGoogleLoginUrl } from "./login-url";

const PENDING_HANDOFF_KEY = "patchbay_desktop_login_handoff";
const PENDING_HANDOFF_TTL_MS = 10 * 60 * 1000;

type PendingHandoff = {
  state: string;
  verifier: string;
  expiresAt: number;
};

export type DesktopHandoffCompletion = {
  /** The callback is terminal and must not be offered to the renderer again. */
  acknowledged: boolean;
  authenticated: boolean;
};

function isPendingHandoff(value: unknown): value is PendingHandoff {
  if (typeof value !== "object" || value === null) return false;
  const candidate = value as Record<string, unknown>;
  return (
    typeof candidate.state === "string" &&
    typeof candidate.verifier === "string" &&
    typeof candidate.expiresAt === "number"
  );
}

function readPendingHandoffs(): PendingHandoff[] {
  const raw = localStorage.getItem(PENDING_HANDOFF_KEY);
  if (!raw) return [];
  try {
    const parsed: unknown = JSON.parse(raw);
    // Accept the previous single-entry shape so an in-flight update does not
    // strand a verifier when the app is upgraded between login attempts.
    const entries = Array.isArray(parsed)
      ? parsed
      : isPendingHandoff(parsed)
        ? [parsed]
        : [];
    const active = entries.filter(
      (entry): entry is PendingHandoff =>
        isPendingHandoff(entry) && entry.expiresAt > Date.now(),
    );
    if (active.length !== entries.length) {
      writePendingHandoffs(active);
    }
    return active;
  } catch {
    return [];
  }
}

function writePendingHandoffs(pending: PendingHandoff[]): void {
  if (pending.length === 0) {
    localStorage.removeItem(PENDING_HANDOFF_KEY);
    return;
  }
  localStorage.setItem(PENDING_HANDOFF_KEY, JSON.stringify(pending));
}

function encodeBase64Url(value: ArrayBuffer): string {
  const bytes = new Uint8Array(value);
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary)
    .replace(/\+/g, "-")
    .replace(/\//g, "_")
    .replace(/=+$/g, "");
}

function randomBase64Url(byteLength: number): string {
  const bytes = new Uint8Array(byteLength);
  crypto.getRandomValues(bytes);
  return encodeBase64Url(bytes.buffer);
}

/**
 * Start a browser-based desktop login without putting a bearer in the custom
 * protocol URL. The verifier remains in app-local storage so a recreated
 * BrowserWindow can redeem the one-time code returned by the web login.
 */
export async function createDesktopGoogleLoginUrl(
  accountsUrl: string,
  browserReturnOrigin?: string,
): Promise<string> {
  const verifier = randomBase64Url(32);
  const digest = await crypto.subtle.digest(
    "SHA-256",
    new TextEncoder().encode(verifier),
  );
  const state = randomBase64Url(32);
  const codeChallenge = encodeBase64Url(digest);
  const pending: PendingHandoff = {
    state,
    verifier,
    expiresAt: Date.now() + PENDING_HANDOFF_TTL_MS,
  };

  const pendingHandoffs = readPendingHandoffs().filter(
    (entry) => entry.state !== state,
  );
  writePendingHandoffs([...pendingHandoffs, pending]);
  const url = new URL(
    buildDesktopGoogleLoginUrl(accountsUrl, browserReturnOrigin),
  );
  url.searchParams.set("code_challenge", codeChallenge);
  url.searchParams.set("state", state);
  return url.href;
}

/** Read the verifier only when the deep-link state matches this renderer. */
export function readDesktopHandoffVerifier(state: string): string | null {
  if (!state) return null;
  return (
    readPendingHandoffs().find((entry) => entry.state === state)?.verifier ??
    null
  );
}

/** Clear a completed handoff without discarding a verifier after a retryable failure. */
export function clearDesktopHandoffVerifier(state: string): void {
  const pending = readPendingHandoffs();
  if (pending.some((entry) => entry.state === state)) {
    writePendingHandoffs(pending.filter((entry) => entry.state !== state));
  }
}

/**
 * Redeem a one-time code, then publish the resulting session. Once redeem
 * succeeds the code can never be used again, so clear its verifier immediately.
 * If user hydration fails after the token was persisted, restart the normal
 * auth initializer instead of attempting to redeem the consumed code again.
 */
export async function completeDesktopHandoff(
  code: string,
  state: string,
  dependencies: {
    redeem: (code: string, verifier: string) => Promise<{ token: string }>;
    login: (token: string) => Promise<unknown>;
    recoverPersistedToken: () => void;
  },
): Promise<DesktopHandoffCompletion> {
  const verifier = readDesktopHandoffVerifier(state);
  if (!verifier) return { acknowledged: true, authenticated: false };

  const { token } = await dependencies.redeem(code, verifier);
  clearDesktopHandoffVerifier(state);
  try {
    await dependencies.login(token);
    return { acknowledged: true, authenticated: true };
  } catch {
    dependencies.recoverPersistedToken();
    return { acknowledged: true, authenticated: false };
  }
}
