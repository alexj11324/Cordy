// Keep in sync with packages/core/auth/desktop-callback-protocol.ts.
// Main must not import `@patchbay/core` (electron-vite externalizes it).

import {
  type DesktopChannel,
  isDesktopCallbackProtocolForChannel,
} from "./desktop-app-identity";

export const PRODUCTION_DESKTOP_CALLBACK_PROTOCOL = "patchbay";

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
  if (typeof v.bundleId !== "string" || !/^ai\.patchbay\.desktop\.canary\.[a-f0-9]{16}$/.test(v.bundleId)
    || v.callbackProtocol !== `patchbay-canary-${v.bundleId.split(".").at(-1)}`
    || typeof v.name !== "string" || !/^Orvilo Canary(?: [a-z0-9-]+)?$/.test(v.name)
    || v.dataName !== v.name.replace("Orvilo", "Patchbay")) {
    throw new Error("Invalid desktop preview identity");
  }
  return v as unknown as DesktopPreviewIdentity;
}

export function resolveDesktopCallbackProtocol(options: {
  channel: DesktopChannel;
  developmentProtocol?: string | null;
  previewIdentity?: DesktopPreviewIdentity | null;
}): string {
  if (options.previewIdentity) return options.previewIdentity.callbackProtocol;
  if (options.channel === "production") return PRODUCTION_DESKTOP_CALLBACK_PROTOCOL;
  // Unpackaged Canary / Staging never claim the global production handler.
  // Each channel must register only its own checkout-owned scheme.
  if (
    !options.developmentProtocol ||
    !isDesktopCallbackProtocolForChannel(
      options.developmentProtocol,
      options.channel,
    )
  ) {
    throw new Error(
      options.channel === "staging"
        ? "Missing or invalid staging callback protocol"
        : "Missing or invalid development callback protocol",
    );
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
