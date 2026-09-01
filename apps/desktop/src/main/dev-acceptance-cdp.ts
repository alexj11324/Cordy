/**
 * Resolve the opt-in Chromium DevTools endpoint used by the credentialed
 * development acceptance runner.
 *
 * Keeping this decision free of Electron imports makes the security boundary
 * unit-testable. The normal dev launcher passes no acceptance flag, and a
 * packaged application can never enable the endpoint through environment
 * variables alone.
 */
export function resolveDevAcceptanceCdpPort({
  isDev,
  isPackaged,
  enabled,
  port,
}: {
  isDev: boolean;
  isPackaged: boolean;
  enabled: string | undefined;
  port: string | undefined;
}): number | null {
  if (!isDev || isPackaged || enabled !== "1") return null;

  const normalized = port?.trim() ?? "";
  if (!/^\d+$/.test(normalized)) {
    throw new Error(
      "PATCHBAY dev acceptance requires a numeric CDP port between 1024 and 65535",
    );
  }

  const value = Number(normalized);
  if (!Number.isInteger(value) || value < 1024 || value > 65535) {
    throw new Error(
      "PATCHBAY dev acceptance requires a numeric CDP port between 1024 and 65535",
    );
  }
  return value;
}
