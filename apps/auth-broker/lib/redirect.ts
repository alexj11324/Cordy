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

/** Keep browser returns inside the broker or its server-validated product origin. */
export function resolveAccountsReturnUrl(
  raw: string | null | undefined,
  productOrigin: string = PRODUCT_ORIGIN,
): string {
  const defaultReturnUrl = new URL("/login", productOrigin).href;
  const value = raw?.trim() ?? "";
  if (!value) return defaultReturnUrl;

  const relative = relativeReturnUrl(value);
  if (relative) return relative;

  try {
    const url = new URL(value);
    if (
      url.protocol === "https:" &&
      url.origin === productOrigin &&
      !url.username &&
      !url.password
    ) {
      return url.href;
    }
  } catch {
    // Invalid values use the product login destination below.
  }

  return defaultReturnUrl;
}

/** Standalone broker login must leave for the product, not loop back to itself. */
export function resolveStandaloneReturnUrl(
  raw: string | null | undefined,
  productOrigin: string = PRODUCT_ORIGIN,
): string {
  const resolved = resolveAccountsReturnUrl(raw, productOrigin);
  return resolved.startsWith("/") ? new URL("/login", productOrigin).href : resolved;
}
