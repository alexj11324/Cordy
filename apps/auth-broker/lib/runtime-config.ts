type RuntimeEnvironment = Record<string, string | undefined>;
export type AuthBrokerRuntimeConfig = { apiOrigin: string; brokerOrigin: string; clerkPublishableKey: string; goBrokerAuthToken: string; originAuthToken: string };
export function readAuthBrokerRuntimeConfig(env: RuntimeEnvironment = process.env): { ok: true; config: AuthBrokerRuntimeConfig } | { ok: false; error: string } {
  try { return { ok: true, config: { apiOrigin: origin(env.PATCHBAY_API_ORIGIN, "PATCHBAY_API_ORIGIN"), brokerOrigin: origin(env.PATCHBAY_AUTH_BROKER_ORIGIN, "PATCHBAY_AUTH_BROKER_ORIGIN"), clerkPublishableKey: text(env.CLERK_PUBLISHABLE_KEY, "CLERK_PUBLISHABLE_KEY"), goBrokerAuthToken: secret(env.PATCHBAY_DESKTOP_BROKER_AUTH_TOKEN, "PATCHBAY_DESKTOP_BROKER_AUTH_TOKEN"), originAuthToken: secret(env.PATCHBAY_ORIGIN_AUTH_TOKEN, "PATCHBAY_ORIGIN_AUTH_TOKEN") } }; }
  catch (error) { return { ok: false, error: error instanceof Error ? error.message : "invalid runtime configuration" }; }
}
function text(value: string | undefined, name: string): string { const out = value?.trim() ?? ""; if (!out || /[\r\n]/.test(out)) throw new Error(`${name} is required`); return out; }
function secret(value: string | undefined, name: string): string { const out = text(value, name); if (!/^[a-f0-9]{64}$/.test(out)) throw new Error(`${name} must be 64 lowercase hexadecimal characters`); return out; }
function origin(value: string | undefined, name: string): string { const url = new URL(text(value, name)); if (url.protocol !== "https:" || url.username || url.password || url.pathname !== "/" || url.search || url.hash) throw new Error(`${name} must be an HTTPS origin`); return url.origin; }
