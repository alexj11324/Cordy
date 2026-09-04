import type { LocalRuntimeProbe } from "./daemon-types";

export const MAX_GUEST_DISPLAY_NAME_LENGTH = 64;
export const MAX_LOCAL_GUEST_PROMPT_LENGTH = 32_768;
export const MAX_LOCAL_GUEST_HISTORY_ENTRIES = 20;
export const MAX_LOCAL_GUEST_OUTPUT_LENGTH = 256_000;
export const DEFAULT_LOCAL_GUEST_TIMEOUT_MS = 10 * 60 * 1000;
export const MAX_LOCAL_GUEST_TIMEOUT_MS = 30 * 60 * 1000;

/**
 * True when the name carries a character that cannot legitimately appear in a
 * display name: C0/C1 controls, DEL, and the two Unicode line separators.
 *
 * Written as a code-point scan rather than a regex so the control range is
 * stated in readable hex instead of escaped literals — and so it iterates by
 * code point, which keeps astral characters intact.
 */
function hasControlCharacters(value: string): boolean {
  for (const character of value) {
    const code = character.codePointAt(0) ?? 0;
    if (
      code <= 0x1f ||
      (code >= 0x7f && code <= 0x9f) ||
      code === 0x2028 ||
      code === 0x2029
    ) {
      return true;
    }
  }
  return false;
}

export interface LocalGuestSession {
  displayName: string;
}

export type LocalGuestMode = "undecided" | "guest" | "cloud";

export type LocalGuestRunRequest = {
  workingDirectory: string;
  prompt: string;
  timeoutMs: number;
};

export type LocalGuestRunEvent = {
  event: "started" | "message" | "result";
  text?: string;
  status?: string;
  error?: string;
  durationMs?: number;
};

export type LocalGuestRunHistoryEntry = {
  id: string;
  prompt: string;
  workingDirectory: string;
  status: string;
  output: string;
  error?: string;
  startedAt: number;
  durationMs?: number;
};

export type LocalGuestRunHistory = {
  lastDirectory?: string;
  runs: LocalGuestRunHistoryEntry[];
};

export type LocalGuestRunStartResult =
  | { ok: true; runId: string }
  | {
      ok: false;
      reason:
        | "unauthorized"
        | "guest_required"
        | "busy"
        | "invalid_request"
        | "invalid_directory"
        | "cli_unavailable"
        | "unavailable";
    };

export type LocalGuestRunCancelResult =
  | { ok: true }
  | {
      ok: false;
      reason: "unauthorized" | "guest_required" | "not_found";
    };

export type LocalGuestRunHistoryResult =
  | { ok: true; history: LocalGuestRunHistory }
  | { ok: false; reason: "unauthorized" | "guest_required" | "unavailable" };

export type GuestSessionReadResult =
  | { ok: true; session: LocalGuestSession | null }
  | {
      ok: false;
      reason: "unauthorized" | "invalid" | "unavailable";
    };

export type GuestSessionMutationResult =
  | { ok: true; session: LocalGuestSession }
  | {
      ok: false;
      reason:
        | "unauthorized"
        | "invalid_name"
        | "guest_active"
        | "cloud_active"
        | "unavailable";
    };

export type GuestSessionClearResult =
  | { ok: true }
  | {
      ok: false;
      reason: "unauthorized" | "cloud_active" | "unavailable";
    };

export type GuestCloudModeResult =
  | { ok: true }
  | {
      ok: false;
      reason: "unauthorized" | "guest_active" | "no_guest" | "unavailable";
    };

export type GuestCloudTeardownResult =
  | { ok: true }
  | { ok: false; reason: "unauthorized" | "not_cloud" | "unavailable" };

/**
 * Normalizes renderer input. The main process calls this again and remains the
 * authority for what is persisted.
 */
export type DesktopStartupMode =
  | "entry"
  | "guest"
  | "guest-error"
  | "cloud";

/**
 * Which surface the renderer should boot into, given main's answer about the
 * Guest session and whether a cloud credential is still on disk.
 *
 * Main remains the authority on cloud access — this only decides what the
 * renderer asks for. The rule it encodes is the one that matters for
 * isolation: a local Guest session beats a stale cloud token, so a machine
 * that was used in Guest mode never boots back into a cloud workspace because
 * an old credential was left behind.
 */
export function resolveDesktopStartupMode(
  guestResult: GuestSessionReadResult,
  hasPersistedCloudToken: boolean,
): DesktopStartupMode {
  if (!guestResult.ok) return "guest-error";
  if (guestResult.session) return "guest";
  return hasPersistedCloudToken ? "cloud" : "entry";
}

export function normalizeGuestDisplayName(value: unknown): string | null {
  if (typeof value !== "string") return null;
  if (hasControlCharacters(value)) return null;
  const normalized = value.normalize("NFC").trim();
  if (!normalized || normalized.length > MAX_GUEST_DISPLAY_NAME_LENGTH) {
    return null;
  }
  if (Array.from(normalized).length > MAX_GUEST_DISPLAY_NAME_LENGTH) {
    return null;
  }
  return normalized;
}

export function parseLocalGuestRunRequest(
  value: unknown,
): LocalGuestRunRequest | null {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    return null;
  }
  const candidate = value as Record<string, unknown>;
  const keys = Object.keys(candidate).sort();
  if (
    keys.length !== 3 ||
    keys[0] !== "prompt" ||
    keys[1] !== "timeoutMs" ||
    keys[2] !== "workingDirectory"
  ) {
    return null;
  }
  if (
    typeof candidate.workingDirectory !== "string" ||
    candidate.workingDirectory.length === 0 ||
    candidate.workingDirectory.length > 4096 ||
    typeof candidate.prompt !== "string" ||
    candidate.prompt.trim().length === 0 ||
    Array.from(candidate.prompt).length > MAX_LOCAL_GUEST_PROMPT_LENGTH ||
    typeof candidate.timeoutMs !== "number" ||
    !Number.isSafeInteger(candidate.timeoutMs) ||
    candidate.timeoutMs < 1_000 ||
    candidate.timeoutMs > MAX_LOCAL_GUEST_TIMEOUT_MS
  ) {
    return null;
  }
  return {
    workingDirectory: candidate.workingDirectory,
    prompt: candidate.prompt,
    timeoutMs: candidate.timeoutMs,
  };
}

export function parseLocalGuestRunEvent(
  value: unknown,
): LocalGuestRunEvent | null {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    return null;
  }
  const candidate = value as Record<string, unknown>;
  if (
    candidate.event !== "started" &&
    candidate.event !== "message" &&
    candidate.event !== "result"
  ) {
    return null;
  }
  for (const key of ["text", "status", "error"]) {
    if (key in candidate && typeof candidate[key] !== "string") return null;
  }
  if (
    "duration_ms" in candidate &&
    (typeof candidate.duration_ms !== "number" ||
      !Number.isSafeInteger(candidate.duration_ms) ||
      candidate.duration_ms < 0)
  ) {
    return null;
  }
  return {
    event: candidate.event,
    ...(typeof candidate.text === "string" ? { text: candidate.text } : {}),
    ...(typeof candidate.status === "string"
      ? { status: candidate.status }
      : {}),
    ...(typeof candidate.error === "string" ? { error: candidate.error } : {}),
    ...(typeof candidate.duration_ms === "number"
      ? { durationMs: candidate.duration_ms }
      : {}),
  };
}

const LOCAL_GUEST_HISTORY_KEYS = new Set(["lastDirectory", "runs"]);
const LOCAL_GUEST_RUN_KEYS = new Set([
  "id",
  "prompt",
  "workingDirectory",
  "status",
  "output",
  "error",
  "startedAt",
  "durationMs",
]);

function hasOnlyKnownKeys(
  candidate: Record<string, unknown>,
  known: ReadonlySet<string>,
): boolean {
  return Object.keys(candidate).every((key) => known.has(key));
}

/**
 * Parses persisted run history without repairing it. An unexpected key means
 * the file was tampered with or written by something that is not this app, so
 * the whole document is refused rather than quietly stripped — a stripped
 * field is a change the user never sees and cannot audit.
 */
export function parseLocalGuestRunHistory(
  value: unknown,
): LocalGuestRunHistory | null {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    return null;
  }
  const candidate = value as Record<string, unknown>;
  if (!hasOnlyKnownKeys(candidate, LOCAL_GUEST_HISTORY_KEYS)) return null;
  if (
    ("lastDirectory" in candidate &&
      (typeof candidate.lastDirectory !== "string" ||
        candidate.lastDirectory.length > 4096)) ||
    !Array.isArray(candidate.runs) ||
    candidate.runs.length > MAX_LOCAL_GUEST_HISTORY_ENTRIES
  ) {
    return null;
  }
  const runs: LocalGuestRunHistoryEntry[] = [];
  for (const value of candidate.runs) {
    if (!value || typeof value !== "object" || Array.isArray(value)) {
      return null;
    }
    const run = value as Record<string, unknown>;
    if (!hasOnlyKnownKeys(run, LOCAL_GUEST_RUN_KEYS)) return null;
    if (
      typeof run.id !== "string" ||
      run.id.length === 0 ||
      run.id.length > 128 ||
      typeof run.prompt !== "string" ||
      Array.from(run.prompt).length > MAX_LOCAL_GUEST_PROMPT_LENGTH ||
      typeof run.workingDirectory !== "string" ||
      run.workingDirectory.length === 0 ||
      run.workingDirectory.length > 4096 ||
      typeof run.status !== "string" ||
      run.status.length === 0 ||
      typeof run.output !== "string" ||
      run.output.length > MAX_LOCAL_GUEST_OUTPUT_LENGTH ||
      ("error" in run && typeof run.error !== "string") ||
      typeof run.startedAt !== "number" ||
      !Number.isSafeInteger(run.startedAt) ||
      ("durationMs" in run &&
        (typeof run.durationMs !== "number" ||
          !Number.isSafeInteger(run.durationMs) ||
          run.durationMs < 0))
    ) {
      return null;
    }
    runs.push({
      id: run.id,
      prompt: run.prompt,
      workingDirectory: run.workingDirectory,
      status: run.status,
      output: run.output,
      ...(typeof run.error === "string" ? { error: run.error } : {}),
      startedAt: run.startedAt,
      ...(typeof run.durationMs === "number"
        ? { durationMs: run.durationMs }
        : {}),
    });
  }
  return {
    ...(typeof candidate.lastDirectory === "string"
      ? { lastDirectory: candidate.lastDirectory }
      : {}),
    runs,
  };
}

/**
 * Parses persisted state without repairing it. A normalized value is required
 * so corrupt or tampered state fails closed instead of being silently changed.
 */
export function parseLocalGuestSession(value: unknown): LocalGuestSession | null {
  if (!value || typeof value !== "object" || Array.isArray(value)) return null;
  const keys = Object.keys(value);
  if (keys.length !== 1 || keys[0] !== "displayName") return null;
  const displayName = (value as { displayName?: unknown }).displayName;
  if (typeof displayName !== "string") return null;
  if (normalizeGuestDisplayName(displayName) !== displayName) return null;
  return { displayName };
}

export function parseLocalRuntimeProbe(value: unknown): LocalRuntimeProbe {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    return { probeResult: "error" };
  }

  const candidate = value as {
    probe_result?: unknown;
    runtime_count?: unknown;
    provider_summary?: unknown;
  };
  if (
    candidate.probe_result !== "success" ||
    !Number.isSafeInteger(candidate.runtime_count) ||
    (candidate.runtime_count as number) < 0 ||
    !candidate.provider_summary ||
    typeof candidate.provider_summary !== "object" ||
    Array.isArray(candidate.provider_summary)
  ) {
    return { probeResult: "error" };
  }

  const providerSummary: Record<string, number> = {};
  for (const [provider, count] of Object.entries(candidate.provider_summary)) {
    if (
      !provider ||
      !Number.isSafeInteger(count) ||
      (count as number) < 0
    ) {
      return { probeResult: "error" };
    }
    providerSummary[provider] = count as number;
  }

  const runtimeCount = candidate.runtime_count as number;
  const summaryCount = Object.values(providerSummary).reduce(
    (sum, count) => sum + count,
    0,
  );
  if (summaryCount !== runtimeCount) return { probeResult: "error" };

  return {
    probeResult: "success",
    runtimeCount,
    providerSummary,
    onlineCount: 0,
    offlineCount: runtimeCount,
  };
}
