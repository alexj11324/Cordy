const DESKTOP_HANDOFF_PARAMS = ["code_challenge", "state"] as const;
const HANDOFF_VALUE_PATTERN = /^[A-Za-z0-9._~-]{43,128}$/;

export type DesktopHandoffBinding = {
  codeChallenge: string;
  state: string;
  query: string;
};

/** Preserve only the canonical PKCE binding through the provider callback. */
export function buildDesktopHandoffQuery(searchParams: URLSearchParams): string {
  const params = new URLSearchParams({ platform: "desktop" });
  for (const key of DESKTOP_HANDOFF_PARAMS) {
    const value = searchParams.get(key);
    if (value) params.set(key, value);
  }
  return params.toString();
}

/**
 * Accept only renderer-generated URL-safe state and PKCE values before
 * starting a provider redirect. The Rust redeem endpoint remains the final
 * authority for the PKCE proof.
 */
export function readDesktopHandoffBinding(
  searchParams: URLSearchParams,
): DesktopHandoffBinding | null {
  if (searchParams.get("platform") !== "desktop") return null;
  // The desktop handoff has one return transport: the patchbay:// custom
  // protocol. Reject the retired HTTP app-origin transport instead of
  // allowing an old link to select a second callback protocol.
  if (searchParams.has("app_origin")) return null;
  const codeChallenge = searchParams.get("code_challenge") ?? "";
  const state = searchParams.get("state") ?? "";
  if (
    !HANDOFF_VALUE_PATTERN.test(codeChallenge) ||
    !HANDOFF_VALUE_PATTERN.test(state)
  ) {
    return null;
  }
  return {
    codeChallenge,
    state,
    query: buildDesktopHandoffQuery(searchParams),
  };
}
