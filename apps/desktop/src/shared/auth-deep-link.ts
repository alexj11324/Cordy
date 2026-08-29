const OPAQUE_CODE_PATTERN = /^[A-Za-z0-9_-]{1,128}$/;

/**
 * Parse the only auth deep-link shape the desktop app accepts. The query must
 * contain exactly one opaque authorization code; bearer-token parameter names
 * and extra parameters are rejected before anything reaches the renderer.
 */
export function parseAuthDeepLinkCode(
  value: string,
  protocols: readonly string[] = ["patchbay", "cordy"],
): string | null {
  try {
    const parsed = new URL(value);
    const protocol = parsed.protocol.slice(0, -1);
    if (!protocols.includes(protocol)) return null;
    if (
      parsed.hostname !== "auth" ||
      parsed.pathname !== "/callback" ||
      parsed.username ||
      parsed.password ||
      parsed.port
    ) {
      return null;
    }
    if (parsed.hash) return null;
    const queryKeys = [...parsed.searchParams.keys()];
    const code = parsed.searchParams.get("code");
    if (queryKeys.length !== 1 || queryKeys[0] !== "code" || !code) {
      return null;
    }
    return OPAQUE_CODE_PATTERN.test(code) ? code : null;
  } catch {
    return null;
  }
}
