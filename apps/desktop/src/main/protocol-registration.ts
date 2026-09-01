export const PROTOCOL = "patchbay";
export const LEGACY_PROTOCOL = "cordy"; // legacy-brand-compat

const DEVELOPMENT_PROTOCOL_PATTERN =
  /^patchbay-canary(?:-[a-z0-9](?:[a-z0-9-]{0,46}[a-z0-9])?)?$/;
export const AUTH_CALLBACK_PROTOCOL_ARG = "--desktop-auth-callback-protocol=";
export const DESKTOP_APP_SUFFIX_ARG = "--desktop-app-suffix=";
const DESKTOP_APP_SUFFIX_PATTERN =
  /^[a-z0-9](?:[a-z0-9-]{0,62}[a-z0-9])?$/;

export function isDesktopCallbackProtocol(value: string): boolean {
  return value === PROTOCOL || DEVELOPMENT_PROTOCOL_PATTERN.test(value);
}

/**
 * The OS may relaunch a development Electron bundle without the environment
 * that started it. The protocol registration therefore carries the exact
 * worktree scheme as a command-line argument; only accept our strict scheme
 * grammar when recovering it from a cold-start argv.
 */
export function readDesktopCallbackProtocol(
  argv: string[],
): string | undefined {
  const argument = argv.find((value) =>
    value.startsWith(AUTH_CALLBACK_PROTOCOL_ARG),
  );
  const protocol = argument?.slice(AUTH_CALLBACK_PROTOCOL_ARG.length);
  return protocol && isDesktopCallbackProtocol(protocol) ? protocol : undefined;
}

export function readDesktopAppSuffix(argv: string[]): string | undefined {
  const argument = argv.find((value) => value.startsWith(DESKTOP_APP_SUFFIX_ARG));
  const suffix = argument?.slice(DESKTOP_APP_SUFFIX_ARG.length);
  return suffix && DESKTOP_APP_SUFFIX_PATTERN.test(suffix) ? suffix : undefined;
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
  desktopAppSuffix?: string;
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
      [
        app.getAppPath(),
        `${AUTH_CALLBACK_PROTOCOL_ARG}${context.authCallbackProtocol}`,
        ...(context.desktopAppSuffix
          ? [`${DESKTOP_APP_SUFFIX_ARG}${context.desktopAppSuffix}`]
          : []),
      ],
    );
    return;
  }

  for (const protocol of [PROTOCOL, LEGACY_PROTOCOL]) {
    app.setAsDefaultProtocolClient(protocol);
  }
}
