import { buildDesktopLoginUrl } from "./login-url";

const PENDING_HANDOFF_KEY = "patchbay_desktop_login_handoff";

type PendingHandoff = {
  state: string;
  verifier: string;
};

function encodeBase64Url(value: ArrayBuffer): string {
  const bytes = new Uint8Array(value);
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/g, "");
}

function randomBase64Url(byteLength: number): string {
  const bytes = new Uint8Array(byteLength);
  crypto.getRandomValues(bytes);
  return encodeBase64Url(bytes.buffer);
}

/**
 * Start a browser-based desktop login without putting a bearer in the custom
 * protocol URL. The verifier remains in this renderer's session storage and
 * is required to redeem the one-time code returned by the web login.
 */
export async function createDesktopLoginUrl(appUrl: string): Promise<string> {
  const verifier = randomBase64Url(32);
  const digest = await crypto.subtle.digest(
    "SHA-256",
    new TextEncoder().encode(verifier),
  );
  const state = randomBase64Url(32);
  const codeChallenge = encodeBase64Url(digest);
  const pending: PendingHandoff = { state, verifier };

  sessionStorage.setItem(PENDING_HANDOFF_KEY, JSON.stringify(pending));
  const url = new URL(buildDesktopLoginUrl(appUrl));
  url.searchParams.set("code_challenge", codeChallenge);
  url.searchParams.set("state", state);
  return url.href;
}

/** Read the verifier only when the deep-link state matches this renderer. */
export function readDesktopHandoffVerifier(state: string): string | null {
  if (!state) return null;
  try {
    const raw = sessionStorage.getItem(PENDING_HANDOFF_KEY);
    if (!raw) return null;
    const pending = JSON.parse(raw) as Partial<PendingHandoff>;
    return pending.state === state && typeof pending.verifier === "string"
      ? pending.verifier
      : null;
  } catch {
    return null;
  }
}

/** Clear a completed handoff without discarding a verifier after a retryable failure. */
export function clearDesktopHandoffVerifier(state: string): void {
  if (readDesktopHandoffVerifier(state)) {
    sessionStorage.removeItem(PENDING_HANDOFF_KEY);
  }
}
