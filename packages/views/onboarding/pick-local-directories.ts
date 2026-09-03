// Desktop-only multi-folder picker for the onboarding projects field.
//
// Mirrors `pickDirectories` from `@patchbay/views/platform` (which the Go
// branch's platform layer does not expose yet): wraps the preload
// `desktopAPI.pickDirectories` surface so the step can SSR-render on web
// (where `window.desktopAPI` is undefined) and degrade gracefully instead
// of crashing. Kept inside onboarding/ because `packages/views/platform/`
// is outside this alignment's scope — once the platform layer gains the
// plural picker, this step should import it from there and drop this file.

export type PickLocalProjectFolder = {
  path: string;
  basename: string;
  originUrl: string | null;
};

export type PickLocalProjectFoldersResult = {
  ok: boolean;
  folders?: PickLocalProjectFolder[];
  reason?: "cancelled" | "no_window" | "error" | "unsupported";
  error?: string;
};

interface DesktopMultiDirectoryAPI {
  pickDirectories?: (
    defaultPath?: string,
  ) => Promise<PickLocalProjectFoldersResult>;
}

function readDesktopAPI(): DesktopMultiDirectoryAPI | undefined {
  if (typeof window === "undefined") return undefined;
  const api = (window as unknown as { desktopAPI?: DesktopMultiDirectoryAPI })
    .desktopAPI;
  return api;
}

/** Multi-folder pick via the desktop preload bridge. Resolves
 *  `{ ok: false, reason: "unsupported" }` on web or in an older desktop
 *  build whose preload only exposes the singular `pickDirectory`. */
export async function pickLocalProjectFolders(
  defaultPath?: string,
): Promise<PickLocalProjectFoldersResult> {
  const api = readDesktopAPI();
  if (!api?.pickDirectories) return { ok: false, reason: "unsupported" };
  return api.pickDirectories(defaultPath);
}
