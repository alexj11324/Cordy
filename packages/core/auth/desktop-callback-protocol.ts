export const PRODUCTION_DESKTOP_CALLBACK_PROTOCOL = "orvilo";

const DEVELOPMENT_PROTOCOL_PATTERN =
  /^orvilo-canary-[a-f0-9]{16}$/;

export function isDesktopCallbackProtocol(value: string): boolean {
  return (
    value === PRODUCTION_DESKTOP_CALLBACK_PROTOCOL ||
    DEVELOPMENT_PROTOCOL_PATTERN.test(value)
  );
}
