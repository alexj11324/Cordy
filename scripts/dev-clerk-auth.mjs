#!/usr/bin/env node

import { execFile as execFileCallback } from "node:child_process";
import { createPublicKey } from "node:crypto";
import { promisify } from "node:util";

const execFile = promisify(execFileCallback);

export const DEFAULT_DEV_CLERK_PROJECT = "general-secrets-store";
export const DEFAULT_DEV_CLERK_SECRET = "patchbay-dev-clerk-auth";

const REMEDIATION =
  "Authenticate gcloud for Secret Manager access, or provide the complete Clerk development variables in the process environment.";

const DEV_CLERK_ENVIRONMENT_KEYS = [
  "NEXT_PUBLIC_CLERK_PUBLISHABLE_KEY",
  "CLERK_PUBLISHABLE_KEY",
  "CLERK_SECRET_KEY",
  "CLERK_JWT_KEY",
  "CLERK_ISSUER",
  "CLERK_AUTHORIZED_PARTIES",
];

function authError(message) {
  return new Error(`${message} ${REMEDIATION}`);
}

function normalizeOrigin(value, label) {
  try {
    const url = new URL(value);
    if (url.protocol !== "https:" && url.protocol !== "http:") {
      throw new Error("unsupported protocol");
    }
    if (url.pathname !== "/" || url.search || url.hash) {
      throw new Error("must be an origin");
    }
    return url.origin;
  } catch {
    throw authError(`${label} must be a valid HTTP(S) origin.`);
  }
}

export function issuerFromPublishableKey(publishableKey) {
  const match = /^(pk_(?:test|live))_(.+)$/.exec(publishableKey || "");
  if (!match) {
    throw authError("Clerk publishable key is missing or invalid.");
  }
  try {
    const encoded = match[2].replace(/-/g, "+").replace(/_/g, "/");
    const decoded = Buffer.from(encoded, "base64").toString("utf8");
    const host = decoded.replace(/\$$/, "");
    if (!host || /[\s/]/.test(host)) throw new Error("invalid host");
    return normalizeOrigin(`https://${host}`, "Derived CLERK_ISSUER");
  } catch (error) {
    if (error?.message?.includes(REMEDIATION)) throw error;
    throw authError("Clerk publishable key cannot be decoded into an issuer.");
  }
}

function isPemPublicKey(value) {
  if (typeof value !== "string" || !value.includes("BEGIN PUBLIC KEY")) {
    return false;
  }
  try {
    createPublicKey(value);
    return true;
  } catch {
    return false;
  }
}

export async function defaultSecretProvider({
  project,
  secret,
  execImpl = execFile,
}) {
  try {
    const { stdout } = await execImpl(
      "gcloud",
      [
        "secrets",
        "versions",
        "access",
        "latest",
        `--project=${project}`,
        `--secret=${secret}`,
      ],
      { encoding: "utf8", maxBuffer: 1024 * 1024, timeout: 10_000 },
    );
    return stdout;
  } catch {
    throw authError(
      `Could not access development Clerk secret '${secret}' in project '${project}'.`,
    );
  }
}

function parseSecretPayload(raw) {
  try {
    const parsed = typeof raw === "string" ? JSON.parse(raw) : raw;
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
      throw new Error("not an object");
    }
    return parsed;
  } catch {
    throw authError("Development Clerk secret must contain a JSON object.");
  }
}

async function jwtKeyFromJwks(issuer, fetchImpl) {
  let response;
  try {
    response = await fetchImpl(`${issuer}/.well-known/jwks.json`, {
      signal: AbortSignal.timeout(10_000),
    });
  } catch {
    throw authError("Could not retrieve Clerk JWKS metadata.");
  }
  if (!response.ok) {
    throw authError(`Clerk JWKS metadata returned HTTP ${response.status}.`);
  }
  let payload;
  try {
    payload = await response.json();
  } catch {
    throw authError("Clerk JWKS metadata was not valid JSON.");
  }
  const candidate = payload?.keys?.find(
    (key) => key?.kty === "RSA" && key?.use === "sig" && key?.n && key?.e,
  );
  if (!candidate) {
    throw authError("Clerk JWKS metadata contains no RSA signing key.");
  }
  try {
    return createPublicKey({ key: candidate, format: "jwk" })
      .export({ type: "spki", format: "pem" })
      .toString();
  } catch {
    throw authError("Clerk JWKS signing key could not be converted to PEM.");
  }
}

export function hasCompleteDevClerkInput(env) {
  return Boolean(
    (env.NEXT_PUBLIC_CLERK_PUBLISHABLE_KEY || env.CLERK_PUBLISHABLE_KEY) &&
      env.CLERK_SECRET_KEY &&
      env.CLERK_JWT_KEY,
  );
}

function hasAnyDevClerkInput(env) {
  return DEV_CLERK_ENVIRONMENT_KEYS.some((key) => {
    const value = env[key];
    return value !== undefined && value !== null && String(value).trim() !== "";
  });
}

export async function bootstrapDevClerkAuth({
  env = process.env,
  fetchImpl = fetch,
  secretProvider = defaultSecretProvider,
} = {}) {
  const project =
    env.PATCHBAY_DEV_CLERK_GSM_PROJECT || DEFAULT_DEV_CLERK_PROJECT;
  const secret =
    env.PATCHBAY_DEV_CLERK_GSM_SECRET || DEFAULT_DEV_CLERK_SECRET;
  const useEnvironment = hasCompleteDevClerkInput(env);
  if (hasAnyDevClerkInput(env) && !useEnvironment) {
    throw authError(
      "Clerk development variables must be provided as a complete set; partial values cannot be combined with Secret Manager credentials.",
    );
  }
  let payload = {};
  if (!useEnvironment) {
    payload = parseSecretPayload(await secretProvider({ project, secret }));
  }

  const credentials = useEnvironment ? env : payload;

  const publicPublishableKey = credentials.NEXT_PUBLIC_CLERK_PUBLISHABLE_KEY;
  const serverPublishableKey = credentials.CLERK_PUBLISHABLE_KEY;
  if (
    publicPublishableKey &&
    serverPublishableKey &&
    publicPublishableKey !== serverPublishableKey
  ) {
    throw authError(
      "NEXT_PUBLIC_CLERK_PUBLISHABLE_KEY and CLERK_PUBLISHABLE_KEY must match when both are provided.",
    );
  }
  const publishableKey = publicPublishableKey || serverPublishableKey;
  const secretKey = credentials.CLERK_SECRET_KEY;
  if (!/^pk_test_/.test(publishableKey || "")) {
    throw authError("Development auth requires a Clerk test publishable key.");
  }
  if (!/^sk_test_/.test(secretKey || "")) {
    throw authError("Development auth requires a Clerk test secret key.");
  }

  const derivedIssuer = issuerFromPublishableKey(publishableKey);
  const configuredIssuer = credentials.CLERK_ISSUER;
  const issuer = configuredIssuer
    ? normalizeOrigin(configuredIssuer, "CLERK_ISSUER")
    : derivedIssuer;
  if (issuer !== derivedIssuer) {
    throw authError(
      "CLERK_ISSUER does not match the configured publishable key.",
    );
  }

  const frontendOrigin = normalizeOrigin(
    env.FRONTEND_ORIGIN || `http://localhost:${env.FRONTEND_PORT || 3000}`,
    "FRONTEND_ORIGIN",
  );
  let jwtKey = credentials.CLERK_JWT_KEY;
  if (!jwtKey) jwtKey = await jwtKeyFromJwks(issuer, fetchImpl);
  if (!isPemPublicKey(jwtKey)) {
    throw authError("CLERK_JWT_KEY is missing or is not a valid public key.");
  }

  const authEnv = {
    NEXT_PUBLIC_CLERK_PUBLISHABLE_KEY: publishableKey,
    CLERK_PUBLISHABLE_KEY: publishableKey,
    CLERK_SECRET_KEY: secretKey,
    CLERK_ISSUER: issuer,
    CLERK_AUTHORIZED_PARTIES: frontendOrigin,
    CLERK_JWT_KEY: jwtKey,
    PATCHBAY_DEV_AUTH_READY: "1",
  };
  return {
    issuer,
    authorizedParties: frontendOrigin,
    source: useEnvironment ? "environment" : "gsm",
    authEnv,
  };
}

export function scopedDevClerkEnvironment(authEnv, scope) {
  if (scope === "web") {
    return {
      NEXT_PUBLIC_CLERK_PUBLISHABLE_KEY:
        authEnv.NEXT_PUBLIC_CLERK_PUBLISHABLE_KEY,
      CLERK_PUBLISHABLE_KEY: authEnv.CLERK_PUBLISHABLE_KEY,
      CLERK_SECRET_KEY: authEnv.CLERK_SECRET_KEY,
      CLERK_ISSUER: authEnv.CLERK_ISSUER,
      CLERK_AUTHORIZED_PARTIES: authEnv.CLERK_AUTHORIZED_PARTIES,
      CLERK_JWT_KEY: authEnv.CLERK_JWT_KEY,
      PATCHBAY_DEV_AUTH_READY: "1",
    };
  }
  if (scope === "backend") return { ...authEnv, PATCHBAY_DEV_AUTH_READY: "1" };
  throw new Error(`Unknown Clerk development auth scope: ${scope}`);
}

export function withoutDevClerkEnvironment(env) {
  const sanitized = { ...env };
  for (const key of [
    "NEXT_PUBLIC_CLERK_PUBLISHABLE_KEY",
    "CLERK_PUBLISHABLE_KEY",
    "CLERK_SECRET_KEY",
    "CLERK_JWT_KEY",
    "CLERK_ISSUER",
    "CLERK_AUTHORIZED_PARTIES",
    "PATCHBAY_DEV_AUTH_READY",
  ]) {
    delete sanitized[key];
  }
  return sanitized;
}
