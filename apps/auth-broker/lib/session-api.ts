const LOOPBACK_HOSTS = new Set(["127.0.0.1", "localhost"]);

export const LOOPBACK_FRESH_PREFIX = "patchbay_desktop_loopback_fresh:";

export function loopbackFreshKey(state: string): string {
  return `${LOOPBACK_FRESH_PREFIX}${state}`;
}

/** Only the developer's own product API may mint a local desktop session. */
export function readLoopbackSessionApi(
  raw: string | null | undefined,
): string | null {
  const value = raw?.trim() ?? "";
  if (!value) return null;
  let url: URL;
  try {
    url = new URL(value);
  } catch {
    return null;
  }
  if (url.protocol !== "http:") return null;
  if (!LOOPBACK_HOSTS.has(url.hostname)) return null;
  if (url.username || url.password) return null;
  if (url.pathname !== "/" && url.pathname !== "") return null;
  if (url.search || url.hash) return null;
  return url.origin;
}

export function desktopSessionCompleteUrl(sessionApi: string): string {
  return `${sessionApi}/auth/desktop-session/complete`;
}
