// Keep in sync with packages/core/auth/desktop-callback-protocol.ts.
// Main must not import `@orvilo/core` (electron-vite externalizes it).

export const PRODUCTION_DESKTOP_CALLBACK_PROTOCOL = "orvilo";
const DEVELOPMENT_DESKTOP_CALLBACK_PROTOCOL =
  /^orvilo-canary-[a-f0-9]{16}$/;

export interface DesktopPreviewIdentity {
  bundleId: string;
  callbackProtocol: string;
  name: string;
  dataName: string;
}

export function parseDesktopPreviewIdentity(value: unknown): DesktopPreviewIdentity | null {
  if (value === undefined) return null;
  if (!value || typeof value !== "object") throw new Error("Invalid desktop preview identity");
  const v = value as Record<string, unknown>;
  if (typeof v.bundleId !== "string" || !/^ai\.orvilo\.desktop\.canary\.[a-f0-9]{16}$/.test(v.bundleId)
    || v.callbackProtocol !== `orvilo-canary-${v.bundleId.split(".").at(-1)}`
    || typeof v.name !== "string" || !/^Orvilo Canary(?: [a-z0-9-]+)?$/.test(v.name)
    || v.dataName !== v.name) {
    throw new Error("Invalid desktop preview identity");
  }
  return v as unknown as DesktopPreviewIdentity;
}

export function resolveDesktopCallbackProtocol(options: {
  packaged: boolean;
  developmentProtocol?: string | null;
  previewIdentity?: DesktopPreviewIdentity | null;
}): string {
  if (options.previewIdentity) return options.previewIdentity.callbackProtocol;
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
