import type { LocalRuntimeProbe } from "./daemon-types";

export const MAX_GUEST_DISPLAY_NAME_LENGTH = 64;

const GUEST_DISPLAY_NAME_CONTROL_CHARACTERS = /[\u0000-\u001f\u007f-\u009f\u2028\u2029]/u;

export interface LocalGuestSession {
  displayName: string;
}

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

/**
 * Normalizes renderer input. The main process calls this again and remains the
 * authority for what is persisted.
 */
export function normalizeGuestDisplayName(value: unknown): string | null {
  if (typeof value !== "string") return null;
  if (GUEST_DISPLAY_NAME_CONTROL_CHARACTERS.test(value)) return null;
  const normalized = value.normalize("NFC").trim();
  if (!normalized || normalized.length > MAX_GUEST_DISPLAY_NAME_LENGTH) {
    return null;
  }
  if (Array.from(normalized).length > MAX_GUEST_DISPLAY_NAME_LENGTH) {
    return null;
  }
  return normalized;
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
