import { AUTH_CONTRACT } from "./contract";

const BROKER_ORIGIN = AUTH_CONTRACT.origins.broker;
const PRODUCT_ORIGIN = AUTH_CONTRACT.origins.product;

export const DEFAULT_ACCOUNTS_RETURN_URL = new URL(
  "/login",
  PRODUCT_ORIGIN,
).href;

function relativeReturnUrl(raw: string): string | null {
  if (!raw.startsWith("/") || raw.startsWith("//") || raw.includes("\\")) {
    return null;
  }

  try {
    const url = new URL(raw, BROKER_ORIGIN);
    if (url.origin !== BROKER_ORIGIN || url.username || url.password) {
      return null;
    }
    return `${url.pathname}${url.search}${url.hash}`;
  } catch {
    return null;
  }
}

/** Keep browser returns inside the broker or the frozen Patchbay product origin. */
export function resolveAccountsReturnUrl(
  raw: string | null | undefined,
): string {
  const value = raw?.trim() ?? "";
  if (!value) return DEFAULT_ACCOUNTS_RETURN_URL;

  const relative = relativeReturnUrl(value);
  if (relative) return relative;

  try {
    const url = new URL(value);
    if (
      url.protocol === "https:" &&
      url.origin === PRODUCT_ORIGIN &&
      !url.username &&
      !url.password
    ) {
      return url.href;
    }
  } catch {
    // Invalid values use the product login destination below.
  }

  return DEFAULT_ACCOUNTS_RETURN_URL;
}

/** Standalone broker login must leave for the product, not loop back to itself. */
export function resolveStandaloneReturnUrl(
  raw: string | null | undefined,
): string {
  const resolved = resolveAccountsReturnUrl(raw);
  return resolved.startsWith("/") ? DEFAULT_ACCOUNTS_RETURN_URL : resolved;
}
