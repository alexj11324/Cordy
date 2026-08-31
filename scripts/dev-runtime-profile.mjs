#!/usr/bin/env node

import { pathToFileURL } from "node:url";

const LOCAL_MODE = "local";
const HOSTED_MODE = "hosted";

const HOSTED_PROFILE = Object.freeze({
  mode: HOSTED_MODE,
  apiUrl: "https://api.aspectlylabs.com",
  wsUrl: "wss://api.aspectlylabs.com/ws",
  appUrl: "https://patchbay.aspectlylabs.com",
  accountsUrl: "https://accounts.aspectlylabs.com",
});

export const DEV_RUNTIME_MODES = Object.freeze({
  LOCAL: LOCAL_MODE,
  HOSTED: HOSTED_MODE,
});

/**
 * Separate the mode flag from arguments intended for electron-vite.
 * `--hosted` is an implementation detail of the complete launcher and must
 * never be forwarded to electron-vite.
 */
export function parseDevRuntimeArgs(argv = []) {
  const electronArgs = [];
  let mode = LOCAL_MODE;
  for (const arg of argv) {
    if (arg === "--hosted") {
      mode = HOSTED_MODE;
      continue;
    }
    electronArgs.push(arg);
  }
  return { mode, electronArgs };
}

export function resolveDevRuntimeProfile(mode, env = {}) {
  if (mode === HOSTED_MODE) return HOSTED_PROFILE;
  if (mode !== LOCAL_MODE) {
    throw new Error(`Unsupported development runtime mode: ${mode}`);
  }

  const backendPort = nonEmpty(env.PORT, "8080");
  const frontendPort = nonEmpty(env.FRONTEND_PORT, "3000");
  return Object.freeze({
    mode: LOCAL_MODE,
    // Keep the browser-facing origin canonical as localhost. The API may
    // remain bound to IPv4, but the OAuth page must not alternate between
    // localhost and 127.0.0.1 because Clerk treats them as different origins.
    apiUrl: `http://127.0.0.1:${backendPort}`,
    wsUrl: `ws://127.0.0.1:${backendPort}/ws`,
    appUrl: `http://localhost:${frontendPort}`,
    accountsUrl: `http://localhost:${frontendPort}`,
  });
}

/** Reject inherited endpoint overrides before the launcher replaces them. */
export function assertDevRuntimeOverridesCompatible(mode, env = {}) {
  const profile = resolveDevRuntimeProfile(mode, env);
  const expected = {
    VITE_API_URL: profile.apiUrl,
    VITE_WS_URL: profile.wsUrl,
    VITE_APP_URL: profile.appUrl,
    VITE_ACCOUNTS_URL: profile.accountsUrl,
  };
  for (const [key, value] of Object.entries(expected)) {
    const inherited = env[key];
    if (
      typeof inherited === "string" &&
      inherited.trim() !== "" &&
      inherited.trim().replace(/\/+$/, "") !== value
    ) {
      throw new Error(
        `${key}=${inherited} conflicts with the ${mode} development runtime profile; unset it and use the matching launcher`,
      );
    }
  }
  return profile;
}

/** Apply one complete profile to every runtime variable consumed by Desktop/Web. */
export function applyDevRuntimeProfile(env, profile) {
  Object.assign(env, {
    PATCHBAY_DEV_MODE: profile.mode,
    PATCHBAY_DEV_API_URL: profile.apiUrl,
    PATCHBAY_DEV_WS_URL: profile.wsUrl,
    PATCHBAY_DEV_APP_URL: profile.appUrl,
    PATCHBAY_DEV_ACCOUNTS_URL: profile.accountsUrl,
    VITE_API_URL: profile.apiUrl,
    VITE_WS_URL: profile.wsUrl,
    VITE_APP_URL: profile.appUrl,
    VITE_ACCOUNTS_URL: profile.accountsUrl,
    NEXT_PUBLIC_API_URL: profile.apiUrl,
    NEXT_PUBLIC_WS_URL: profile.wsUrl,
  });
  return env;
}

function nonEmpty(value, fallback) {
  return typeof value === "string" && value.trim() !== "" ? value : fallback;
}

if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(process.argv[1]).href
) {
  const { mode } = parseDevRuntimeArgs(process.argv.slice(2));
  const profile = resolveDevRuntimeProfile(mode, process.env);
  process.stdout.write(`${JSON.stringify(profile)}\n`);
}
