/** Production CSRF cookie. Staging hosts also receive this from `.aspectlylabs.com`. */
export const PRODUCTION_CSRF_COOKIE_NAME = "orvilo_csrf";

/**
 * Staging CSRF cookie. The shared production Web image cannot bake a
 * build-time name, so the client prefers this name when both cookies are
 * present. Production hosts never receive `.staging.aspectlylabs.com`
 * cookies, so this is a no-op there. The Go server only issues this name
 * or PRODUCTION_CSRF_COOKIE_NAME; unknown CSRF_COOKIE_NAME values fall
 * back to the production default.
 */
export const STAGING_CSRF_COOKIE_NAME = "orvilo_staging_csrf";

const CSRF_COOKIE_NAMES = [STAGING_CSRF_COOKIE_NAME, PRODUCTION_CSRF_COOKIE_NAME] as const;

/** Read the CSRF token from a `document.cookie` / Cookie header string. */
export function readCsrfTokenFromCookieHeader(header: string): string | null {
  if (header.trim() === "") return null;
  const cookies = header.split(";").map((part) => part.trim());
  for (const name of CSRF_COOKIE_NAMES) {
    const prefix = `${name}=`;
    const match = cookies.find((cookie) => cookie.startsWith(prefix));
    if (!match) continue;
    const value = match.slice(prefix.length);
    if (value !== "") return value;
  }
  return null;
}
