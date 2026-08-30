const DESKTOP_HANDOFF_PARAMS = ["code_challenge", "state"] as const;

/** Preserve the binding that lets a desktop OAuth callback return safely. */
export function buildDesktopHandoffQuery(
  searchParams: URLSearchParams,
): string {
  const params = new URLSearchParams({ platform: "desktop" });
  for (const key of DESKTOP_HANDOFF_PARAMS) {
    const value = searchParams.get(key);
    if (value) params.set(key, value);
  }
  return params.toString();
}
