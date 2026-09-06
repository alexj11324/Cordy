/**
 * Development sign-in for the Electron app.
 *
 * Desktop authenticates with a bearer token in renderer localStorage, not with
 * the HttpOnly cookie the web app uses — so the URL `make dev-login` prints
 * signs you into the browser and does nothing for Electron. `make up C=desktop`
 * mints a token from /auth/dev-login and writes it into the gitignored
 * apps/desktop/.env.development.local as VITE_DEV_LOGIN_TOKEN; this seeds it
 * into storage at boot so the app starts signed in.
 *
 * Kept as a pure function so the rules that matter — never clobber a real
 * session, always replace a stale seeded one — are testable without a renderer.
 */

/** The subset of the Storage API this needs, so tests need no DOM. */
export interface TokenStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

export const DESKTOP_TOKEN_KEY = "patchbay_token";

/**
 * Records the value this helper last seeded. It is what tells a session the
 * developer created (by signing in through the app) apart from one this helper
 * put there: only the latter may be replaced.
 */
export const DESKTOP_SEEDED_TOKEN_KEY = "patchbay_dev_seeded_token";

/**
 * Seeds the development token into storage. Returns whether it wrote one.
 *
 * A session the developer established always wins — the seed must never sign
 * someone out of the account they are testing with. A previously seeded token
 * is replaced instead of kept: it expires, and recreating the local database
 * invalidates it, after which the API rejects it and the app would sit on the
 * login screen until a second restart even though a fresh token was available
 * all along.
 */
export function seedDevLoginToken(
  storage: TokenStorage,
  token: unknown,
): boolean {
  if (typeof token !== "string") return false;
  const trimmed = token.trim();
  if (trimmed === "") return false;

  try {
    const stored = storage.getItem(DESKTOP_TOKEN_KEY);
    if (stored) {
      if (stored !== storage.getItem(DESKTOP_SEEDED_TOKEN_KEY)) return false;
      if (stored === trimmed) return false;
    }
    storage.setItem(DESKTOP_TOKEN_KEY, trimmed);
    storage.setItem(DESKTOP_SEEDED_TOKEN_KEY, trimmed);
    return true;
  } catch {
    // Storage can throw when the renderer blocks site data. A dev convenience
    // must never take the app's boot down with it.
    return false;
  }
}
