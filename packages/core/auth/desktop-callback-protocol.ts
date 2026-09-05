export const PRODUCTION_DESKTOP_CALLBACK_PROTOCOL = "patchbay";

const DEVELOPMENT_PROTOCOL_PATTERN =
  /^patchbay-canary-[a-f0-9]{16}$/;

export function isDesktopCallbackProtocol(value: string): boolean {
  return (
    value === PRODUCTION_DESKTOP_CALLBACK_PROTOCOL ||
    DEVELOPMENT_PROTOCOL_PATTERN.test(value)
  );
}
