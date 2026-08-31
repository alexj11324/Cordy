type RuntimeEnvironment = Record<string, string | undefined>;

export type AuthBrokerRuntimeConfig = {
  apiOrigin: string;
  brokerOrigin: string;
  clerkPublishableKey: string;
};

export type RuntimeConfigResult =
  | { ok: true; config: AuthBrokerRuntimeConfig }
  | { ok: false; error: string };

export function readAuthBrokerRuntimeConfig(
  env: RuntimeEnvironment = process.env,
): RuntimeConfigResult {
  try {
    const apiOrigin = requiredOrigin(env.PATCHBAY_API_ORIGIN, "PATCHBAY_API_ORIGIN");
    const brokerOrigin = requiredOrigin(
      env.PATCHBAY_AUTH_BROKER_ORIGIN,
      "PATCHBAY_AUTH_BROKER_ORIGIN",
    );
    const clerkPublishableKey = requiredString(
      env.CLERK_PUBLISHABLE_KEY,
      "CLERK_PUBLISHABLE_KEY",
    );
    return {
      ok: true,
      config: { apiOrigin, brokerOrigin, clerkPublishableKey },
    };
  } catch (error) {
    return {
      ok: false,
      error: error instanceof Error ? error.message : "invalid runtime configuration",
    };
  }
}

function requiredString(value: string | undefined, name: string): string {
  const normalized = value?.trim() ?? "";
  if (!normalized) throw new Error(`${name} is required`);
  if (/[\r\n]/.test(normalized)) throw new Error(`${name} is invalid`);
  return normalized;
}

function requiredOrigin(value: string | undefined, name: string): string {
  const raw = requiredString(value, name);
  let url: URL;
  try {
    url = new URL(raw);
  } catch {
    throw new Error(`${name} must be an HTTPS origin`);
  }
  const localDevelopment =
    process.env.NODE_ENV !== "production" &&
    url.protocol === "http:" &&
    (url.hostname === "localhost" || url.hostname === "127.0.0.1");
  if (
    (!localDevelopment && url.protocol !== "https:") ||
    url.username ||
    url.password ||
    url.pathname !== "/" ||
    url.search ||
    url.hash
  ) {
    throw new Error(`${name} must be an HTTPS origin`);
  }
  return url.origin;
}
