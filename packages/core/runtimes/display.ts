import type { AgentRuntime } from "../types";

/** Product surface: a Device is the machine; a Harness is the CLI on it. */
export type DeviceKind = "desktop" | "laptop" | "terminal";

/** Split a daemon-baked name such as `Codex (host)` into its two parts. */
export function splitRuntimeName(name: string): {
  base: string;
  hostname: string | null;
} {
  const match = name.match(/^(.+?)\s+\(([^)]+)\)$/);
  if (!match || !match[1] || !match[2]) {
    return { base: name, hostname: null };
  }
  return { base: match[1], hostname: match[2] };
}

export interface LocalMachineMatch {
  localDaemonId?: string | null;
  localMachineName?: string | null;
  currentUserId?: string | null;
}

/**
 * True when this runtime is the viewing user's current computer.
 * Prefer the daemon UUID. Fall back to host name only for that user's
 * own local runtimes — the workspace list includes everyone else's
 * machines, and host names are not unique.
 */
export function isCurrentLocalRuntime(
  runtime: Pick<
    AgentRuntime,
    "runtime_mode" | "daemon_id" | "name" | "device_info" | "owner_id"
  >,
  options: LocalMachineMatch,
): boolean {
  if (runtime.runtime_mode !== "local") return false;
  if (options.localDaemonId && runtime.daemon_id === options.localDaemonId) {
    return true;
  }
  const localName = options.localMachineName?.trim();
  if (!localName || !options.currentUserId) return false;
  if (runtime.owner_id !== options.currentUserId) return false;
  const needle = localName.toLowerCase();
  if (runtime.daemon_id?.toLowerCase() === needle) return true;
  const host = deviceHostName(runtime);
  return !!host && host.toLowerCase() === needle;
}

/** Device name for the viewing user: "this machine" when it is theirs. */
export function deviceLabelForViewer(
  runtime: Pick<
    AgentRuntime,
    | "runtime_mode"
    | "daemon_id"
    | "name"
    | "device_info"
    | "owner_id"
    | "custom_name"
    | "provider"
  >,
  options: LocalMachineMatch,
  thisMachineLabel: string,
): string {
  if (isCurrentLocalRuntime(runtime, options)) return thisMachineLabel;
  return deviceDisplayName(runtime);
}

/** Return the registered machine name without the Harness prefix. */
export function deviceHostName(
  runtime: Pick<AgentRuntime, "name" | "device_info">,
): string | null {
  const host = splitRuntimeName(runtime.name).hostname;
  if (host) return host;
  const raw = runtime.device_info?.trim();
  if (!raw) return null;
  return raw.split(" · ")[0]?.trim() || null;
}

/** User-facing machine name; never render the Harness name as the device. */
export function deviceDisplayName(
  runtime: Pick<
    AgentRuntime,
    "name" | "custom_name" | "device_info" | "runtime_mode" | "provider"
  >,
): string {
  const host = deviceHostName(runtime);
  if (host) return host;

  const custom = runtime.custom_name?.trim();
  const provider = runtime.provider?.trim() ?? "";
  const providerLabel = provider ? providerDisplayName(provider) : "";
  const name = runtime.name.trim();
  const looksLikeHarnessOnly =
    !!name &&
    (name.toLowerCase() === provider.toLowerCase() || name === providerLabel);

  if (name && !looksLikeHarnessOnly) return name;
  if (custom) return custom;
  if (runtime.runtime_mode === "cloud") {
    return providerLabel ? `${providerLabel} cloud` : "Cloud";
  }
  return "Local";
}

/** User-facing Harness family name. */
export function harnessDisplayName(
  runtime: Pick<AgentRuntime, "provider" | "name">,
): string {
  const provider = runtime.provider?.trim();
  if (provider) return providerDisplayName(provider);
  return splitRuntimeName(runtime.name).base;
}

const LAPTOP_RE =
  /macbook|laptop|notebook|thinkpad|xps|chromebook|surface laptop/;

/** Pick a neutral device icon family from the runtime registration. */
export function deviceKind(
  runtime: Pick<AgentRuntime, "runtime_mode" | "device_info" | "name">,
): DeviceKind {
  if (runtime.runtime_mode === "cloud") return "terminal";
  const blob = `${runtime.device_info ?? ""} ${runtime.name}`.toLowerCase();
  if (LAPTOP_RE.test(blob)) return "laptop";
  return "desktop";
}

/**
 * The name to show for a runtime (MUL-4217): the user's custom override when
 * set, otherwise the daemon-proposed default. Defends against older backends
 * that omit custom_name and against whitespace-only overrides.
 */
export function runtimeDisplayName(
  runtime: Pick<AgentRuntime, "name" | "custom_name">,
): string {
  const custom = runtime.custom_name?.trim();
  return custom ? custom : runtime.name;
}

/**
 * A runtime label that always surfaces the provider family, even when a custom
 * alias is set (#5260). The daemon bakes the provider into `name`
 * ("Codex (host)"), but a user alias replaces that whole string via
 * runtimeDisplayName and hides which CLI actually backs the runtime. When an
 * alias is present we re-attach the provider in parentheses; without one `name`
 * already carries it, so we return it unchanged to avoid a duplicated provider
 * ("Codex (host) (codex)").
 */
export function runtimeDisplayLabel(
  runtime: Pick<AgentRuntime, "name" | "custom_name" | "provider">,
): string {
  const display = runtimeDisplayName(runtime);
  const hasCustom = !!runtime.custom_name?.trim();
  const provider = runtime.provider?.trim();
  if (!hasCustom || !provider) return display;
  return `${display} (${providerDisplayName(provider)})`;
}

/**
 * Provider slugs whose human display name isn't just a capitalization of the
 * slug. This MUST mirror the daemon's `runtimeDisplayNameOverrides`
 * (server/internal/daemon/daemon.go): the daemon bakes that display name into
 * `name` for the no-alias case (for example, "Trae (host)"), so the aliased label has to use
 * the exact same names or the two paths drift apart (#5260). `qoderclicn`,
 * `codearts`, `dsh`, `traecli`, `qwen`, `qwenpaw`, `mcode`, and `zeroclaw` need overrides today — every other provider is a
 * first-letter capitalization of its slug on both sides. Keep in sync with the daemon map.
 */
const PROVIDER_DISPLAY_NAMES: Record<string, string> = {
  codearts: "CodeArts",
  dsh: "DeepSeek Harness",
  qoderclicn: "Qoder CN",
  traecli: "Trae",
  qwen: "Qwen Code",
  qwenpaw: "QwenPaw",
  mcode: "MiniMax Code",
  omp: "Oh-My-Pi",
  zeroclaw: "ZeroClaw",
};

/**
 * Map a provider slug to its display name, matching the daemon's
 * providerDisplayName: an explicit override when listed, otherwise the slug
 * with its first letter capitalized.
 */
export function providerDisplayName(provider: string): string {
  const known = PROVIDER_DISPLAY_NAMES[provider];
  if (known) return known;
  return provider.charAt(0).toUpperCase() + provider.slice(1);
}
