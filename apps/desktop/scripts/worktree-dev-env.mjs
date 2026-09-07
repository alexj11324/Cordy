// Per-worktree dev isolation for `pnpm dev:desktop`.
//
// Two `pnpm dev:desktop` instances from two different git worktrees collide on
// the renderer Vite port (5173) and the single-instance lock / userData dir
// (keyed by the app name "Orvilo Canary"). The env hooks to override both
// already exist — electron.vite.config.ts reads DESKTOP_RENDERER_PORT and
// src/main/index.ts reads DESKTOP_APP_SUFFIX — but nothing derives unique
// values per worktree. This module does, mirroring the offset scheme that
// scripts/init-worktree-env.sh already uses for backend/frontend ports and
// adding a collision-resistant callback scheme derived from the full path.
//
// Backend targeting is deliberately NOT touched here: which backend the desktop
// connects to stays driven by apps/desktop/.env* (VITE_API_URL / VITE_WS_URL),
// exactly as documented. This module adds the process identity knobs needed
// for Electron instances and their OS callbacks to coexist.

import { createHash } from "node:crypto";
import { statSync } from "node:fs";
import { basename, join, resolve } from "node:path";

// Worktree renderer ports start at 5174 so they never reuse 5173 — the primary
// checkout's default — even when a worktree's offset is 0 (e.g. POSIX cksum of
// "/tmp/orvilo-3494" is 1189739000, and 1189739000 % 1000 === 0). Range 5174–6173.
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

export function rendererPortForPath(path, channel = "development") {
  // A disjoint 15174–16173 block has no Chromium-restricted ports.
  if (channel === "staging") return 15174 + offsetForPath(path);
  return avoidRestrictedPort(RENDERER_PORT_BASE + offsetForPath(path));
}

export function identityHashForPath(path) {
  return createHash("sha256")
    .update(resolve(path))
    .digest("hex")
    .slice(0, 16);
}

// Worktree → a readable, collision-resistant, filesystem-safe suffix.
// The path hash keeps userData and the single-instance lock unique even when
// two worktrees share a basename or the 1000-slot port allocator wraps.
export function appSuffixForPath(path) {
  const slug =
    basename(path)
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-+|-+$/g, "") || "worktree";
  return `${slug}-${identityHashForPath(path).slice(0, 12)}`;
}

// The OS callback identity must not share the 1000-slot port namespace. A
// truncated SHA-256 of the full app path stays stable for this checkout and
// makes collisions between arbitrary worktree locations negligible. Keep the
// hash in sync with apps/desktop/src/shared/desktop-app-identity.ts.
export function callbackProtocolForPath(appPath, channel = "development") {
  const prefix = channel === "staging" ? "orvilo-staging" : "orvilo-canary";
  return `${prefix}-${identityHashForPath(appPath)}`;
}

// A linked git worktree has a `.git` FILE (a "gitdir:" pointer); the primary
// checkout has a `.git` DIRECTORY. We only auto-isolate linked worktrees, so
// the primary checkout keeps the unchanged 5173 / "Orvilo Canary" defaults.
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

// Populate DESKTOP_RENDERER_PORT / DESKTOP_APP_SUFFIX for a linked worktree and
// DESKTOP_CALLBACK_PROTOCOL for every development checkout, without overriding
// explicit values. Returns `env`.
export function applyWorktreeDevEnv(env, { root, log = false } = {}) {
  const hasPort = Boolean(env.DESKTOP_RENDERER_PORT);
  const hasSuffix = Boolean(env.DESKTOP_APP_SUFFIX);
  const linked = isLinkedWorktree(root);
  const channel = env.ORVILO_DESKTOP_CHANNEL === "staging" ? "staging" : "development";

  if (!hasPort && (linked || channel === "staging")) {
    env.DESKTOP_RENDERER_PORT = String(
      linked ? rendererPortForPath(root, channel) : 15173,
    );
  }
  if (linked && !hasSuffix) env.DESKTOP_APP_SUFFIX = appSuffixForPath(root);
  // Callback ownership is not an override knob: letting ambient shell state
  // choose it can make two otherwise isolated checkouts claim one OS scheme.
  env.DESKTOP_CALLBACK_PROTOCOL = callbackProtocolForPath(
    join(root, "apps", "desktop"),
    channel,
  );

  if (log) {
    const display = channel === "staging" ? "Orvilo Staging" : "Orvilo Canary";
    const appName = env.DESKTOP_APP_SUFFIX
      ? `${display} ${env.DESKTOP_APP_SUFFIX}`
      : display;
    const renderer = env.DESKTOP_RENDERER_PORT ?? "5173";
    console.log(
      `[dev:desktop] checkout isolation → renderer port ${renderer}, ` +
        `app "${appName}", callback ${env.DESKTOP_CALLBACK_PROTOCOL}://`,
    );
  }
  return env;
}
