import type { User } from "@patchbay/core/types";

export const GUEST_TOKEN_PREFIX = "pbg_";
const GUEST_TOKEN_HEX_LENGTH = 40;

const GUEST_TOKEN_PATTERN = new RegExp(
  `^${GUEST_TOKEN_PREFIX}[0-9a-fA-F]{${GUEST_TOKEN_HEX_LENGTH}}$`,
);
const UUID_PATTERN =
  /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;

export type GuestSessionStatus = "active" | "claimed" | "revoked";

export type GuestSession = {
  id: string;
  user_id: string;
  status: GuestSessionStatus;
  created_at: string;
  claimed_at: string | null;
  claimed_by: string | null;
};

/** The public guest auth response. `session_id` is optional for compatibility
 * with servers that predate the response metadata; logout still revokes the
 * active bearer through `/auth/logout`. */
export type GuestAuthResponse = {
  token: string;
  user: User;
  session_id?: string;
};

export function assertGuestToken(value: string): void {
  if (!isGuestToken(value)) {
    throw new Error("Invalid guest token");
  }
}

export function isGuestToken(value: unknown): value is string {
  return typeof value === "string" && GUEST_TOKEN_PATTERN.test(value);
}

/** Keep lifecycle IDs in a path segment, never concatenate attacker input. */
export function guestSessionPath(
  sessionId: string,
  action?: "claim" | "revoke",
): string {
  if (!UUID_PATTERN.test(sessionId)) {
    throw new Error("Invalid guest session id");
  }
  const suffix = action ? `/${action}` : "";
  return `/api/guest-sessions/${encodeURIComponent(sessionId)}${suffix}`;
}
