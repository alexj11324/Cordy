export const PROTOCOL = "patchbay";
export const LEGACY_PROTOCOL = "cordy"; // legacy-brand-compat

const DEVELOPMENT_PROTOCOL_PATTERN =
  /^patchbay-canary(?:-[a-z0-9](?:[a-z0-9-]{0,46}[a-z0-9])?)?$/;

function isDesktopCallbackProtocol(value: string): boolean {
  return value === PROTOCOL || DEVELOPMENT_PROTOCOL_PATTERN.test(value);
}

type ProtocolClientRegistrar = {
  getAppPath: () => string;
  setAsDefaultProtocolClient: (
    protocol: string,
    path?: string,
    args?: string[],
  ) => boolean;
};

type ProtocolRegistrationContext = {
  isDefaultApp: boolean;
  platform: NodeJS.Platform;
  execPath: string;
  authCallbackProtocol: string;
};

export function findDesktopProtocolUrl(
  argv: string[],
  authCallbackProtocol: string,
): string | undefined {
  const protocols = new Set([PROTOCOL, LEGACY_PROTOCOL]);
  if (isDesktopCallbackProtocol(authCallbackProtocol)) {
    protocols.add(authCallbackProtocol);
  }
  return argv.find((argument) =>
    [...protocols].some((protocol) => argument.startsWith(`${protocol}://`)),
  );
}

/**
 * Register browser callback schemes without letting a development Electron
 * host replace the installed Patchbay app. Development uses a per-worktree
 * scheme; the macOS dev branding step gives that Electron bundle a matching
 * unique identifier so LaunchServices can return to the initiating instance.
 */
export function registerDesktopProtocolClients(
  app: ProtocolClientRegistrar,
  context: ProtocolRegistrationContext,
): void {
  if (context.isDefaultApp) {
    if (
      !isDesktopCallbackProtocol(context.authCallbackProtocol) ||
      context.authCallbackProtocol === PROTOCOL
    ) {
      throw new Error("Invalid development desktop auth callback protocol");
    }
    app.setAsDefaultProtocolClient(
      context.authCallbackProtocol,
      context.execPath,
      [app.getAppPath()],
    );
    return;
  }

  for (const protocol of [PROTOCOL, LEGACY_PROTOCOL]) {
    app.setAsDefaultProtocolClient(protocol);
  }
}
