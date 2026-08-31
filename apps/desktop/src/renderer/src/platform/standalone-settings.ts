import { isReservedSlug } from "@patchbay/core/paths";

/**
 * Settings is a window-level destination on desktop, not a tab session.
 * Keep the matcher independent from React/store code so both the navigation
 * boundary and tab-state sanitizer enforce the same rule.
 */
export function standaloneSettingsWorkspaceSlug(path: string): string | null {
  const pathname = path.split(/[?#]/, 1)[0] ?? "";
  const segments = pathname.split("/").filter(Boolean);
  if (
    segments.length === 2 &&
    !isReservedSlug(segments[0] ?? "") &&
    segments[1] === "settings"
  ) {
    return segments[0] ?? null;
  }
  return null;
}

export function isStandaloneSettingsPath(path: string): boolean {
  return standaloneSettingsWorkspaceSlug(path) !== null;
}
