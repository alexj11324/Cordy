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
 * session, never write a junk value — are testable without a renderer.
 */

/** The subset of the Storage API this needs, so tests need no DOM. */
export interface TokenStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

export const DESKTOP_TOKEN_KEY = "patchbay_token";

/**
 * Seeds the development token into storage. Returns whether it wrote one.
 *
 * An existing token always wins: the seed is a starting point for a fresh
 * profile, not an override that would log a developer out of the account they
 * are actually testing with.
 */
export function seedDevLoginToken(
  storage: TokenStorage,
  token: unknown,
): boolean {
  if (typeof token !== "string") return false;
  const trimmed = token.trim();
  if (trimmed === "") return false;

  try {
    if (storage.getItem(DESKTOP_TOKEN_KEY)) return false;
    storage.setItem(DESKTOP_TOKEN_KEY, trimmed);
    return true;
  } catch {
    // Storage can throw when the renderer blocks site data. A dev convenience
    // must never take the app's boot down with it.
    return false;
  }
}
