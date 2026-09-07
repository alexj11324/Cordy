import { AUTH_CONTRACT } from "./contract";
type RuntimeEnvironment = Record<string, string | undefined>;
export type AuthBrokerRuntimeConfig = { apiOrigin: string; brokerOrigin: string; productOrigin: string; clerkPublishableKey: string; goBrokerAuthToken: string; originAuthToken: string };
export function readAuthBrokerRuntimeConfig(env: RuntimeEnvironment = process.env): { ok: true; config: AuthBrokerRuntimeConfig } | { ok: false; error: string } {
  try {
    const brokerOrigin = origin(env.ORVILO_AUTH_BROKER_ORIGIN, "ORVILO_AUTH_BROKER_ORIGIN");
    return { ok: true, config: { apiOrigin: origin(env.ORVILO_API_ORIGIN, "ORVILO_API_ORIGIN"), brokerOrigin, productOrigin: productOriginForBroker(env.ORVILO_PRODUCT_ORIGIN, brokerOrigin), clerkPublishableKey: text(env.CLERK_PUBLISHABLE_KEY, "CLERK_PUBLISHABLE_KEY"), goBrokerAuthToken: secret(env.ORVILO_DESKTOP_BROKER_AUTH_TOKEN, "ORVILO_DESKTOP_BROKER_AUTH_TOKEN"), originAuthToken: secret(env.ORVILO_ORIGIN_AUTH_TOKEN, "ORVILO_ORIGIN_AUTH_TOKEN") } };
  }
  catch (error) { return { ok: false, error: error instanceof Error ? error.message : "invalid runtime configuration" }; }
}
function text(value: string | undefined, name: string): string { const out = value?.trim() ?? ""; if (!out || /[\r\n]/.test(out)) throw new Error(`${name} is required`); return out; }
function secret(value: string | undefined, name: string): string { const out = text(value, name); if (!/^[a-f0-9]{64}$/.test(out)) throw new Error(`${name} must be 64 lowercase hexadecimal characters`); return out; }
function origin(value: string | undefined, name: string): string { const url = new URL(text(value, name)); if (url.protocol !== "https:" || url.username || url.password || url.pathname !== "/" || url.search || url.hash) throw new Error(`${name} must be an HTTPS origin`); return url.origin; }
function productOriginForBroker(value: string | undefined, brokerOrigin: string): string {
  const raw = value?.trim() ?? "";
  if (brokerOrigin === AUTH_CONTRACT.origins.broker) {
    return raw ? origin(raw, "ORVILO_PRODUCT_ORIGIN") : AUTH_CONTRACT.origins.product;
  }
  if (!raw) {
    throw new Error("ORVILO_PRODUCT_ORIGIN must override the production product origin when the broker origin is not production");
  }
  const productOrigin = origin(raw, "ORVILO_PRODUCT_ORIGIN");
  if (productOrigin === AUTH_CONTRACT.origins.product) {
    throw new Error("ORVILO_PRODUCT_ORIGIN must not be the production product origin when the broker origin is not production");
  }
  return productOrigin;
}
