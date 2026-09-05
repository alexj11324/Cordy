// Keep in sync with packages/core/auth/desktop-callback-protocol.ts.
// Main must not import `@patchbay/core` (electron-vite externalizes it).

export const PRODUCTION_DESKTOP_CALLBACK_PROTOCOL = "patchbay";
const DEVELOPMENT_DESKTOP_CALLBACK_PROTOCOL =
  /^patchbay-canary-[a-f0-9]{16}$/;

export function resolveDesktopCallbackProtocol(options: {
  packaged: boolean;
  developmentProtocol?: string | null;
}): string {
  if (options.packaged) return PRODUCTION_DESKTOP_CALLBACK_PROTOCOL;
  if (
    !options.developmentProtocol ||
    !DEVELOPMENT_DESKTOP_CALLBACK_PROTOCOL.test(options.developmentProtocol)
  ) {
    throw new Error("Missing or invalid development callback protocol");
  }
  return options.developmentProtocol;
}

export function isDesktopDeepLink(url: string, protocol: string): boolean {
  try {
    return new URL(url).protocol === `${protocol}:`;
  } catch {
    return false;
  }
}
