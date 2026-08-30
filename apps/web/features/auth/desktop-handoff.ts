const DESKTOP_HANDOFF_PARAMS = ["code_challenge", "state"] as const;
const HANDOFF_VALUE_PATTERN = /^[A-Za-z0-9._~-]{43,128}$/;

export type DesktopHandoffBinding = {
  codeChallenge: string;
  state: string;
  appOrigin: string | null;
  query: string;
};

function normalizeAppOrigin(value: string | null | undefined): string | null {
  if (!value) return null;
  try {
    const url = new URL(value);
    if (
      (url.protocol !== "http:" && url.protocol !== "https:") ||
      url.username ||
      url.password ||
      url.pathname !== "/" ||
      url.search ||
      url.hash
    ) {
      return null;
    }
    return url.origin;
  } catch {
    return null;
  }
}

/** Accept a browser return only when deployment config names that exact origin. */
export function readDesktopBrowserAppOrigin(
  searchParams: URLSearchParams,
  configuredOrigin: string | undefined,
): string | null {
  const requested = normalizeAppOrigin(searchParams.get("app_origin"));
  const configured = normalizeAppOrigin(configuredOrigin);
  return requested && configured && requested === configured
    ? requested
    : null;
}

/** Preserve the binding that lets a desktop OAuth callback return safely. */
export function buildDesktopHandoffQuery(
  searchParams: URLSearchParams,
  configuredAppOrigin?: string,
): string {
  const params = new URLSearchParams({ platform: "desktop" });
  for (const key of DESKTOP_HANDOFF_PARAMS) {
    const value = searchParams.get(key);
    if (value) params.set(key, value);
  }
  const appOrigin = readDesktopBrowserAppOrigin(
    searchParams,
    configuredAppOrigin,
  );
  if (appOrigin) params.set("app_origin", appOrigin);
  return params.toString();
}

/**
 * Accept only renderer-generated URL-safe state and PKCE values before
 * starting a provider redirect. The Rust redeem endpoint remains the final
 * authority for the PKCE proof.
 */
export function readDesktopHandoffBinding(
  searchParams: URLSearchParams,
  configuredAppOrigin?: string,
): DesktopHandoffBinding | null {
  if (searchParams.get("platform") !== "desktop") return null;
  const codeChallenge = searchParams.get("code_challenge") ?? "";
  const state = searchParams.get("state") ?? "";
  if (
    !HANDOFF_VALUE_PATTERN.test(codeChallenge) ||
    !HANDOFF_VALUE_PATTERN.test(state)
  ) {
    return null;
  }
  const requestedAppOrigin = searchParams.get("app_origin");
  const appOrigin = readDesktopBrowserAppOrigin(
    searchParams,
    configuredAppOrigin,
  );
  if (requestedAppOrigin !== null && appOrigin === null) return null;
  return {
    codeChallenge,
    state,
    appOrigin,
    query: buildDesktopHandoffQuery(searchParams, configuredAppOrigin),
  };
}

/** Return a PKCE-bound one-time code to an explicitly allowlisted app origin. */
export function redirectToDesktopBrowserApp(
  appOrigin: string,
  code: string,
  state: string,
): void {
  const callback = new URL("/auth/callback", appOrigin);
  callback.searchParams.set("code", code);
  callback.searchParams.set("state", state);
  window.location.replace(callback.href);
}
