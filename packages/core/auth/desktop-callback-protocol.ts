export const PRODUCTION_DESKTOP_CALLBACK_PROTOCOL = "patchbay";
export const DEVELOPMENT_DESKTOP_CALLBACK_PROTOCOL = "patchbay-canary";

const DEVELOPMENT_PROTOCOL_PATTERN =
  /^patchbay-canary(?:-[a-z0-9](?:[a-z0-9-]{0,46}[a-z0-9])?)?$/;

export function isDesktopCallbackProtocol(value: string): boolean {
  return (
    value === PRODUCTION_DESKTOP_CALLBACK_PROTOCOL ||
    DEVELOPMENT_PROTOCOL_PATTERN.test(value)
  );
}
