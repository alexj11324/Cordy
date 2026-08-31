import { homedir } from "os";
import { join } from "path";
import { DEFAULT_RUNTIME_CONFIG } from "../shared/runtime-config";

// Keep this in sync with patchbay_daemon::control_client::health_port_for_profile.
export const DEFAULT_HEALTH_PORT = 19514;
const LEGACY_PACKAGED_API_URL = "https://api.patchbay.ai";

export type LegacyDesktopProfile = {
  name: string;
  serverUrl: string;
};

/**
 * Return the one Desktop-owned profile that must be retired when an installed
 * app moves from the previous packaged API host to the canonical host. Custom
 * and self-hosted endpoint switches deliberately do not stop other profiles.
 */
export function legacyDesktopProfileForTarget(
  targetUrl: string,
): LegacyDesktopProfile | null {
  try {
    const target = new URL(targetUrl);
    const canonical = new URL(DEFAULT_RUNTIME_CONFIG.apiUrl);
    if (
      target.origin !== canonical.origin ||
      target.pathname.replace(/\/+$/, "") !==
        canonical.pathname.replace(/\/+$/, "") ||
      target.search ||
      target.hash
    ) {
      return null;
    }
    return {
      name: deriveProfileName(LEGACY_PACKAGED_API_URL),
      serverUrl: LEGACY_PACKAGED_API_URL,
    };
  } catch {
    return null;
  }
}

/**
 * Desktop owns only `~/.patchbay/profiles/desktop-<host>/`. The default profile
 * at `~/.patchbay/` — config, daemon log, and health port 19514 — belongs to the
 * user's terminal CLI and must never be read, written, probed, or passed to the
 * bundled CLI.
 *
 * Callers signal "target API URL not known yet" with `null`, never with an
 * empty name, so there is no value that can quietly resolve to the default
 * profile. This guard is the backstop for that. See #6399.
 */
export function assertResolvedProfile(profile: string): void {
  if (!profile) {
    throw new Error(
      "daemon profile is unresolved — refusing to fall back to the default CLI profile",
    );
  }
}

// Desktop owns a dedicated CLI profile named after the target API host, so it
// never reads or writes the user's hand-configured profiles. Profile dir:
//   ~/.patchbay/profiles/desktop-<host>/
export function deriveProfileName(targetUrl: string): string {
  try {
    const url = new URL(targetUrl);
    const host = url.host.replace(/:/g, "-").toLowerCase();
    return `desktop-${host}`;
  } catch {
    return "desktop";
  }
}

/**
 * Port 19514 itself is the default profile's, so an unresolved profile must
 * never produce one: probing it would report the user's own CLI daemon as
 * Desktop's, and lifecycle commands would act on it.
 */
export function healthPortForProfile(profile: string): number {
  assertResolvedProfile(profile);
  let sum = 0;
  for (const b of Buffer.from(profile, "utf-8")) sum += b;
  return DEFAULT_HEALTH_PORT + 1 + (sum % 1000);
}

export function profileDir(profile: string): string {
  assertResolvedProfile(profile);
  return join(homedir(), ".patchbay", "profiles", profile);
}

export function profileConfigPath(profile: string): string {
  return join(profileDir(profile), "config.json");
}

export function profileLogPath(profile: string): string {
  return join(profileDir(profile), "daemon.log");
}

// Legacy sidecar retained only so the startup hardening pass can restrict files
// written by older Desktop versions. Current credentials store the owner id in
// the same atomic config.json replacement as the PAT; runtime code must not use
// this path as an authority for token reuse.
export function profileUserIdPath(profile: string): string {
  return join(profileDir(profile), ".desktop-user-id");
}

/**
 * CLI args selecting the Desktop-owned profile. An unresolved profile must
 * never produce an empty arg list: the bundled CLI would then act on the
 * user's default profile at `~/.patchbay/config.json`.
 */
export function profileArgs(profile: string): string[] {
  assertResolvedProfile(profile);
  return ["--profile", profile];
}
