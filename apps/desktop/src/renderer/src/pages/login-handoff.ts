import { ApiError } from "@patchbay/core/api";

const PENDING_HANDOFF_KEY = "patchbay_desktop_login_handoff";
const PENDING_HANDOFF_TTL_MS = 10 * 60 * 1000;

type PendingHandoff = {
  state: string;
  verifier: string;
  expiresAt: number;
};

export type DesktopHandoffCompletion = {
  acknowledged: boolean;
  authenticated: boolean;
};

function isTerminalRedeemFailure(error: unknown): boolean {
  if (!(error instanceof ApiError)) return false;
  return (
    error.status >= 400 &&
    error.status < 500 &&
    error.status !== 408 &&
    error.status !== 429
  );
}

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
    const entries = Array.isArray(parsed)
      ? parsed
      : isPendingHandoff(parsed)
        ? [parsed]
        : [];
    const active = entries.filter(
      (entry): entry is PendingHandoff =>
        isPendingHandoff(entry) && entry.expiresAt > Date.now(),
    );
    if (active.length !== entries.length) writePendingHandoffs(active);
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

/** Register a PKCE binding, then build the browser login URL. */
export async function createDesktopGoogleLoginUrl(
  appUrl: string,
  initiate: (
    state: string,
    codeChallenge: string,
  ) => Promise<{ registered: boolean }>,
): Promise<string> {
  const verifier = randomBase64Url(32);
  const digest = await crypto.subtle.digest(
    "SHA-256",
    new TextEncoder().encode(verifier),
  );
  const state = randomBase64Url(32);
  const codeChallenge = encodeBase64Url(digest);

  const { registered } = await initiate(state, codeChallenge);
  if (!registered) throw new Error("Desktop login handoff was rejected");

  const pendingHandoffs = readPendingHandoffs().filter(
    (entry) => entry.state !== state,
  );
  writePendingHandoffs([
    ...pendingHandoffs,
    { state, verifier, expiresAt: Date.now() + PENDING_HANDOFF_TTL_MS },
  ]);

  const url = new URL(`${appUrl.replace(/\/+$/, "")}/login`);
  url.searchParams.set("platform", "desktop");
  url.searchParams.set("state", state);
  url.searchParams.set("code_challenge", codeChallenge);
  return url.href;
}

function readDesktopHandoffVerifier(state: string): string | null {
  if (!state) return null;
  return (
    readPendingHandoffs().find((entry) => entry.state === state)?.verifier ??
    null
  );
}

function clearDesktopHandoffVerifier(state: string): void {
  const pending = readPendingHandoffs();
  if (pending.some((entry) => entry.state === state)) {
    writePendingHandoffs(pending.filter((entry) => entry.state !== state));
  }
}

/** Redeem the one-time code and establish the native bearer session. */
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

  let token: string;
  try {
    ({ token } = await dependencies.redeem(code, verifier));
  } catch (error) {
    if (isTerminalRedeemFailure(error)) {
      clearDesktopHandoffVerifier(state);
      return { acknowledged: true, authenticated: false };
    }
    throw error;
  }

  clearDesktopHandoffVerifier(state);
  try {
    await dependencies.login(token);
    return { acknowledged: true, authenticated: true };
  } catch {
    dependencies.recoverPersistedToken();
    return { acknowledged: true, authenticated: false };
  }
}
