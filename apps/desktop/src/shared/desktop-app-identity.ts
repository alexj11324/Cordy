export type DesktopChannel = "development" | "staging" | "production";

export interface DesktopAppIdentity {
  channel: DesktopChannel;
  name: string;
  userDataDirName: string;
  appUserModelId: string;
  bundleIdPrefix: string;
  isolateUserData: boolean;
}

export interface DesktopAppIdentityInput {
  isDev: boolean;
  mode?: string;
  suffix?: string | undefined;
}

export function resolveDesktopChannel(
  options: Pick<DesktopAppIdentityInput, "isDev" | "mode">,
): DesktopChannel {
  if (!options.isDev) return "production";
  if (options.mode === "staging") return "staging";
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
      name: "Patchbay",
      userDataDirName: "Patchbay",
      appUserModelId: "ai.patchbay.desktop",
      bundleIdPrefix: "ai.patchbay.desktop",
      isolateUserData: false,
    };
  }
  if (channel === "staging") {
    const name = withOptionalSuffix("Patchbay Staging", options.suffix);
    return {
      channel,
      name,
      userDataDirName: name,
      appUserModelId: "ai.patchbay.desktop.staging",
      bundleIdPrefix: "ai.patchbay.desktop.staging",
      isolateUserData: true,
    };
  }
  const name = withOptionalSuffix("Patchbay Canary", options.suffix);
  return {
    channel,
    name,
    userDataDirName: name,
    appUserModelId: "ai.patchbay.desktop.dev",
    bundleIdPrefix: "ai.patchbay.desktop.canary",
    isolateUserData: true,
  };
}

export function desktopChannelFromArgv(argv: readonly string[]): DesktopChannel {
  const modeIndex = argv.indexOf("--mode");
  if (modeIndex >= 0 && argv[modeIndex + 1] === "staging") return "staging";
  return "development";
}
