#!/usr/bin/env node

import { execFile as execFileCallback } from "node:child_process";
import { createPublicKey } from "node:crypto";
import { promisify } from "node:util";

const execFile = promisify(execFileCallback);

export const DEFAULT_DEV_CLERK_PROJECT = "general-secrets-store";
export const DEFAULT_DEV_CLERK_SECRET = "patchbay-dev-clerk-auth";

const REMEDIATION =
  "Authenticate gcloud for Secret Manager access, or provide the complete Clerk development variables in the process environment.";

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

async function defaultSecretProvider({ project, secret }) {
  try {
    const { stdout } = await execFile(
      "gcloud",
      [
        "secrets",
        "versions",
        "access",
        "latest",
        `--project=${project}`,
        `--secret=${secret}`,
      ],
      { encoding: "utf8", maxBuffer: 1024 * 1024 },
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

export async function bootstrapDevClerkAuth({
  env = process.env,
  fetchImpl = fetch,
  secretProvider = defaultSecretProvider,
} = {}) {
  const project =
    env.PATCHBAY_DEV_CLERK_GSM_PROJECT || DEFAULT_DEV_CLERK_PROJECT;
  const secret =
    env.PATCHBAY_DEV_CLERK_GSM_SECRET || DEFAULT_DEV_CLERK_SECRET;
  let payload = {};
  if (!hasCompleteDevClerkInput(env)) {
    payload = parseSecretPayload(await secretProvider({ project, secret }));
  }

  const publishableKey =
    env.NEXT_PUBLIC_CLERK_PUBLISHABLE_KEY ||
    env.CLERK_PUBLISHABLE_KEY ||
    payload.NEXT_PUBLIC_CLERK_PUBLISHABLE_KEY ||
    payload.CLERK_PUBLISHABLE_KEY;
  const secretKey = env.CLERK_SECRET_KEY || payload.CLERK_SECRET_KEY;
  if (!/^sk_(?:test|live)_/.test(secretKey || "")) {
    throw authError("CLERK_SECRET_KEY is missing or invalid.");
  }

  const derivedIssuer = issuerFromPublishableKey(publishableKey);
  const configuredIssuer = env.CLERK_ISSUER || payload.CLERK_ISSUER;
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
  let jwtKey = env.CLERK_JWT_KEY || payload.CLERK_JWT_KEY;
  if (!jwtKey) jwtKey = await jwtKeyFromJwks(issuer, fetchImpl);
  if (!isPemPublicKey(jwtKey)) {
    throw authError("CLERK_JWT_KEY is missing or is not a valid public key.");
  }

  Object.assign(env, {
    NEXT_PUBLIC_CLERK_PUBLISHABLE_KEY: publishableKey,
    CLERK_PUBLISHABLE_KEY: publishableKey,
    CLERK_SECRET_KEY: secretKey,
    CLERK_ISSUER: issuer,
    CLERK_AUTHORIZED_PARTIES: frontendOrigin,
    CLERK_JWT_KEY: jwtKey,
    PATCHBAY_DEV_AUTH_READY: "1",
  });
  return {
    issuer,
    authorizedParties: frontendOrigin,
    source: Object.keys(payload).length > 0 ? "gsm" : "environment",
  };
}
