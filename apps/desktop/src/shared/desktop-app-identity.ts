import { createHash } from "node:crypto";
import { resolve } from "node:path";

export type DesktopChannel = "development" | "staging" | "production";

export const PRODUCTION_DESKTOP_CALLBACK_PROTOCOL_PREFIX = "orvilo";
export const STAGING_DESKTOP_CALLBACK_PROTOCOL_PREFIX = "orvilo-staging";
export const DEVELOPMENT_DESKTOP_CALLBACK_PROTOCOL_PREFIX = "orvilo-canary";

const CHANNEL_CALLBACK_PROTOCOL_PATTERN: Record<DesktopChannel, RegExp> = {
  production: /^orvilo$/,
  staging: /^orvilo-staging-[a-f0-9]{16}$/,
  development: /^orvilo-canary-[a-f0-9]{16}$/,
};

export interface DesktopAppIdentity {
  channel: DesktopChannel;
  name: string;
  userDataDirName: string;
  appUserModelId: string;
  bundleIdPrefix: string;
  callbackProtocolPrefix: string;
  isolateUserData: boolean;
}

export interface DesktopAppIdentityInput {
  isDev: boolean;
  mode?: string;
  argv?: readonly string[];
  suffix?: string | undefined;
}

export function callbackProtocolPrefixForChannel(
  channel: DesktopChannel,
): string {
  if (channel === "staging") return STAGING_DESKTOP_CALLBACK_PROTOCOL_PREFIX;
  if (channel === "development") return DEVELOPMENT_DESKTOP_CALLBACK_PROTOCOL_PREFIX;
  return PRODUCTION_DESKTOP_CALLBACK_PROTOCOL_PREFIX;
}

export function isDesktopCallbackProtocolForChannel(
  protocol: string,
  channel: DesktopChannel,
): boolean {
  return CHANNEL_CALLBACK_PROTOCOL_PATTERN[channel].test(protocol);
}

export function identityHashForPath(path: string): string {
  return createHash("sha256").update(resolve(path)).digest("hex").slice(0, 16);
}

export function checkoutCallbackProtocol(
  channel: DesktopChannel,
  appPath: string,
): string {
  const prefix = callbackProtocolPrefixForChannel(channel);
  if (channel === "production") return prefix;
  return `${prefix}-${identityHashForPath(appPath)}`;
}

export function protocolClientLaunchArgs(
  appPath: string,
  channel: DesktopChannel,
): string[] {
  // OS-launched protocol handlers start Electron without the vite MODE env.
  // Staging must re-enter as `--mode staging` or the new process becomes Canary
  // and redeems a staging callback against the wrong userData / API.
  if (channel === "staging") return [appPath, "--mode", "staging"];
  return [appPath];
}

export function resolveDesktopChannel(
  options: Pick<DesktopAppIdentityInput, "isDev" | "mode" | "argv">,
): DesktopChannel {
  if (!options.isDev) return "production";
  if (
    options.mode === "staging" ||
    desktopChannelFromArgv(options.argv ?? []) === "staging"
  ) {
    return "staging";
  }
  return "development";
}

function withOptionalSuffix(base: string, suffix: string | undefined): string {
  const trimmed = suffix?.trim();
  return trimmed ? `${base} ${trimmed}` : base;
}

export function resolveDesktopAppIdentity(
  options: DesktopAppIdentityInput,
): DesktopAppIdentity {
  const channel = resolveDesktopChannel(options);
  if (channel === "production") {
    return {
      channel,
      name: "Orvilo",
      // The identity cutover starts with an Orvilo-owned profile directory.
      userDataDirName: "Orvilo",
      appUserModelId: "ai.orvilo.desktop",
      bundleIdPrefix: "ai.orvilo.desktop",
      callbackProtocolPrefix: callbackProtocolPrefixForChannel(channel),
      isolateUserData: false,
    };
  }
  if (channel === "staging") {
    return {
      channel,
      name: withOptionalSuffix("Orvilo Staging", options.suffix),
      userDataDirName: withOptionalSuffix("Orvilo Staging", options.suffix),
      appUserModelId: "ai.orvilo.desktop.staging",
      bundleIdPrefix: "ai.orvilo.desktop.staging",
      callbackProtocolPrefix: callbackProtocolPrefixForChannel(channel),
      isolateUserData: true,
    };
  }
  return {
    channel,
    name: withOptionalSuffix("Orvilo Canary", options.suffix),
    userDataDirName: withOptionalSuffix("Orvilo Canary", options.suffix),
    appUserModelId: "ai.orvilo.desktop.dev",
    bundleIdPrefix: "ai.orvilo.desktop.canary",
    callbackProtocolPrefix: callbackProtocolPrefixForChannel(channel),
    isolateUserData: true,
  };
}

export function desktopChannelFromArgv(argv: readonly string[]): DesktopChannel {
  const modeIndex = argv.indexOf("--mode");
  if (modeIndex >= 0 && argv[modeIndex + 1] === "staging") return "staging";
  return "development";
}
