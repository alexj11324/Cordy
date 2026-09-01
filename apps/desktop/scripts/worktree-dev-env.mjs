// Per-worktree dev isolation for `pnpm dev:desktop`.
//
// Two `pnpm dev:desktop` instances from two different git worktrees collide on
// the renderer Vite port (5173) and the single-instance lock / userData dir
// (keyed by the app name "Patchbay Canary"). The env hooks to override both
// already exist — electron.vite.config.ts reads DESKTOP_RENDERER_PORT and
// src/main/index.ts reads DESKTOP_APP_SUFFIX — but nothing derives unique
// values per worktree. This module does, mirroring the offset scheme that
// scripts/init-worktree-env.sh already uses for backend/frontend ports.
//
// Backend targeting is deliberately NOT touched here: which backend the desktop
// connects to stays driven by apps/desktop/.env* (VITE_API_URL / VITE_WS_URL),
// exactly as documented. This module only adds the two knobs needed for two
// Electron processes to coexist.

import { createHash } from "node:crypto";
import { statSync } from "node:fs";
import { basename, join, resolve } from "node:path";

// Worktree renderer ports start at 5174 so they never reuse 5173 — the primary
// checkout's default — even when a worktree's offset is 0 (e.g. POSIX cksum of
// "/tmp/patchbay-3494" is 1189739000, and 1189739000 % 1000 === 0). Range 5174–6173.
const RENDERER_PORT_BASE = 5174;
const OFFSET_MODULO = 1000;

// Chromium refuses to navigate to a URL on its restricted-port list and fails
// the load with ERR_UNSAFE_PORT, so a worktree whose derived port lands on one
// gets a blank Electron window instead of the renderer -- the Vite server is
// up and healthy, which makes it read as a renderer bug rather than a port one.
// Exactly one restricted port falls inside 5174-6173: 6000 (X11). Those ports
// are remapped, in list order, into the block immediately above the range, so
// the offset -> port mapping stays injective and two worktrees still cannot
// collide. Keep this sorted and in sync with net::kRestrictedPorts.
const RESTRICTED_PORTS_IN_RANGE = [6000];

function avoidRestrictedPort(port) {
  const index = RESTRICTED_PORTS_IN_RANGE.indexOf(port);
  return index === -1 ? port : RENDERER_PORT_BASE + OFFSET_MODULO + index;
}

export function rendererPortForOffset(offset) {
  return avoidRestrictedPort(RENDERER_PORT_BASE + offset);
}

// POSIX cksum (CRC-32), kept byte-compatible with `cksum(1)` so the offset
// matches scripts/init-worktree-env.sh — a worktree's backend (18080+offset),
// frontend (13000+offset) and desktop renderer (5174+offset) ports all share
// one offset. Verified against coreutils: cksum of "/tmp/foo" → 427878967.
function cksumTable() {
  const table = new Uint32Array(256);
  const POLY = 0x04c11db7;
  for (let i = 0; i < 256; i++) {
    let crc = i << 24;
    for (let bit = 0; bit < 8; bit++) {
      crc = crc & 0x80000000 ? (crc << 1) ^ POLY : crc << 1;
    }
    table[i] = crc >>> 0;
  }
  return table;
}

const TABLE = cksumTable();

export function cksum(buf) {
  let crc = 0;
  for (const byte of buf) {
    crc = (((crc << 8) >>> 0) ^ TABLE[((crc >>> 24) ^ byte) & 0xff]) >>> 0;
  }
  // POSIX appends the byte length, least-significant byte first.
  let len = buf.length;
  while (len > 0) {
    crc = (((crc << 8) >>> 0) ^ TABLE[((crc >>> 24) ^ (len & 0xff)) & 0xff]) >>> 0;
    len = Math.floor(len / 256);
  }
  return (~crc) >>> 0;
}

export function offsetForPath(path) {
  return cksum(Buffer.from(path)) % OFFSET_MODULO;
}

/** Stable checkout identity used in names that must not collide across clones. */
export function checkoutIdentity(path) {
  return createHash("sha256")
    .update(resolve(path))
    .digest("hex")
    .slice(0, 16);
}

export function rendererPortForPath(path) {
  return rendererPortForOffset(offsetForPath(path));
}

// Worktree → a readable, filesystem-safe suffix containing the full checkout
// identity prefix and the port offset. The identity is required because two
// independent clones can have the same basename and the same 0–999 offset.
// The dev app then gets its own userData / single-instance lock under a name
// such as "Patchbay Canary patchbay-a1b2c3d4-194".
export function appSuffixForPath(path) {
  const slug =
    basename(path)
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-+|-+$/g, "") || "worktree";
  return `${slug}-${checkoutIdentity(path).slice(0, 8)}-${offsetForPath(path)}`;
}

export function appSuffixForOffset(path, offset) {
  const slug =
    basename(path)
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-+|-+$/g, "") || "worktree";
  return `${slug}-${checkoutIdentity(path).slice(0, 8)}-${offset}`;
}

const AUTH_CALLBACK_PROTOCOL_PREFIX = "patchbay-canary";
const AUTH_CALLBACK_SUFFIX_MAX_LENGTH = 48;

export function authCallbackProtocolForSuffix(suffix) {
  const normalized = String(suffix ?? "")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
  if (!normalized) return AUTH_CALLBACK_PROTOCOL_PREFIX;
  if (normalized.length <= AUTH_CALLBACK_SUFFIX_MAX_LENGTH) {
    return `${AUTH_CALLBACK_PROTOCOL_PREFIX}-${normalized}`;
  }

  const offset = normalized.match(/-\d+$/)?.[0] ?? "";
  const availablePrefixLength = AUTH_CALLBACK_SUFFIX_MAX_LENGTH - offset.length;
  const bounded = `${normalized.slice(0, availablePrefixLength).replace(/-+$/g, "")}${offset}`;
  return `${AUTH_CALLBACK_PROTOCOL_PREFIX}-${bounded}`;
}

export function devElectronDistPath({
  home,
  authCallbackProtocol,
  electronVersion,
  arch,
}) {
  if (!home || !authCallbackProtocol || !electronVersion || !arch) {
    throw new Error("Incomplete development Electron path inputs");
  }
  return join(
    home,
    "Applications",
    "Patchbay Development",
    authCallbackProtocol,
    `${electronVersion}-${arch}`,
  );
}

export function applyMacOSDevElectronEnv(
  env,
  {
    home,
    electronVersion,
    arch = process.arch,
    platform = process.platform,
  } = {},
) {
  if (platform !== "darwin") return env;
  env.PATCHBAY_DEV_ELECTRON_DIST_PATH = devElectronDistPath({
    home,
    authCallbackProtocol: env.DESKTOP_AUTH_CALLBACK_PROTOCOL,
    electronVersion,
    arch,
  });
  return env;
}

// A linked git worktree has a `.git` FILE (a "gitdir:" pointer); the primary
// checkout has a `.git` DIRECTORY. We only auto-isolate linked worktrees, so
// the primary checkout keeps the unchanged 5173 / "Patchbay Canary" defaults.
export function isLinkedWorktree(root) {
  try {
    return statSync(join(root, ".git")).isFile();
  } catch {
    return false;
  }
}

// scripts live at <root>/apps/desktop/scripts
export function repoRootFromScriptDir(scriptDir) {
  return join(scriptDir, "..", "..", "..");
}

// Populate the renderer port, app suffix, and isolated auth callback protocol
// without overriding values the caller set explicitly. Returns `env`.
export function applyWorktreeDevEnv(env, { root, log = false } = {}) {
  const hasPort = Boolean(env.DESKTOP_RENDERER_PORT);
  const hasSuffix = Boolean(env.DESKTOP_APP_SUFFIX);
  const hasAuthCallbackProtocol = Boolean(env.DESKTOP_AUTH_CALLBACK_PROTOCOL);
  const linkedWorktree = isLinkedWorktree(root);

  if (linkedWorktree) {
    if (!hasPort) env.DESKTOP_RENDERER_PORT = String(rendererPortForPath(root));
    if (!hasSuffix) env.DESKTOP_APP_SUFFIX = appSuffixForPath(root);
  }
  if (!hasAuthCallbackProtocol) {
    env.DESKTOP_AUTH_CALLBACK_PROTOCOL = authCallbackProtocolForSuffix(
      env.DESKTOP_APP_SUFFIX,
    );
  }

  if (log && linkedWorktree) {
    console.log(
      `[dev:desktop] worktree isolation → renderer port ${env.DESKTOP_RENDERER_PORT}, ` +
        `app "Patchbay Canary ${env.DESKTOP_APP_SUFFIX}", ` +
        `callback ${env.DESKTOP_AUTH_CALLBACK_PROTOCOL}://`,
    );
  }
  return env;
}
