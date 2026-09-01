/**
 * Explicit Patchbay runtime provider → @lobehub/icons Avatar id.
 *
 * Do not use `@lobehub/icons` `AgentIcon` keyword matching: it maps `kiro` to
 * `KiloCode`. Every shipping provider is listed here so a new runtime cannot
 * silently fall through to a wrong brand.
 */
export const ACP_LOBEHUB_ICON_BY_PROVIDER = {
  claude: "ClaudeCode",
  codebuddy: "CodeBuddy",
  codex: "Codex",
  copilot: "Copilot",
  opencode: "OpenCode",
  // DevEco is Huawei's IDE; lobehub has no DevEco mark.
  deveco: "Huawei",
  openclaw: "OpenClaw",
  hermes: "HermesAgent",
  pi: "Pi",
  omp: "Pi",
  cursor: "Cursor",
  kimi: "Kimi",
  dsh: "DeepSeek",
  kiro: "Kiro",
  antigravity: "Antigravity",
  qoder: "Qoder",
  qoderclicn: "Qoder",
  traecli: "Trae",
  grok: "Grok",
  qwen: "Qwen",
  qwenpaw: "Qwen",
  mcode: "Minimax",
} as const;

export type AcpLobehubIconId =
  (typeof ACP_LOBEHUB_ICON_BY_PROVIDER)[keyof typeof ACP_LOBEHUB_ICON_BY_PROVIDER];

/**
 * Every Patchbay runtime provider id, including the `omp` builtin. Keep in
 * lockstep with `PROVIDERS` + `BUILTIN_RUNTIMES` in
 * `server-rs/crates/patchbay-agent/src/registry.rs`.
 */
export const ALL_RUNTIME_PROVIDERS = [
  "claude",
  "codebuddy",
  "codex",
  "copilot",
  "opencode",
  "deveco",
  "openclaw",
  "hermes",
  "pi",
  "cursor",
  "kimi",
  "reasonix",
  "dsh",
  "kiro",
  "antigravity",
  "qoder",
  "qoderclicn",
  "traecli",
  "grok",
  "qwen",
  "qwenpaw",
  "mcode",
  "dim",
  "omp",
] as const;

export type RuntimeProviderId = (typeof ALL_RUNTIME_PROVIDERS)[number];

export type AcpAvatarSource =
  | { kind: "lobehub"; id: AcpLobehubIconId; provider: string }
  | { kind: "fallback"; provider: string }
  | { kind: "none" };

export function acpAvatarSource(
  provider: string | null | undefined,
): AcpAvatarSource {
  if (!provider) return { kind: "none" };
  const id =
    ACP_LOBEHUB_ICON_BY_PROVIDER[
      provider as keyof typeof ACP_LOBEHUB_ICON_BY_PROVIDER
    ];
  if (id) return { kind: "lobehub", id, provider };
  return { kind: "fallback", provider };
}

export function resolveAgentProvider(
  agentId: string | undefined,
  agents: readonly { id: string; runtime_id: string }[],
  runtimes: readonly { id: string; provider: string }[],
): string | null {
  if (!agentId) return null;
  const agent = agents.find((entry) => entry.id === agentId);
  if (!agent) return null;
  return runtimes.find((runtime) => runtime.id === agent.runtime_id)?.provider ?? null;
}
