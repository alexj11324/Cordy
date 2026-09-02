import { describe, expect, it } from "vitest";

import { GuestAuthResponseSchema, GuestSessionSchema } from "./schemas";
import { assertGuestToken, guestSessionPath, isGuestToken } from "./guest-auth";

const guestToken = `pbg_${"a".repeat(40)}`;
const sessionId = "018f03a0-c4d2-7a37-ae4d-5aa45de12f11";

describe("mobile guest auth contract", () => {
  it("accepts only the server's opaque pbg token shape", () => {
    expect(isGuestToken(guestToken)).toBe(true);
    expect(isGuestToken(`pbg_${"A".repeat(40)}`)).toBe(true);
    expect(isGuestToken(`PBG_${"a".repeat(40)}`)).toBe(false);
    expect(isGuestToken(`pbg_${"a".repeat(39)}`)).toBe(false);
    expect(isGuestToken(`jwt_${"a".repeat(40)}`)).toBe(false);
    expect(isGuestToken(undefined)).toBe(false);
    expect(() => assertGuestToken("not-a-guest-token")).toThrow(
      "Invalid guest token",
    );
  });

  it("keeps v7 session IDs in safe lifecycle paths", () => {
    expect(guestSessionPath(sessionId)).toBe(
      `/api/guest-sessions/${sessionId}`,
    );
    expect(guestSessionPath(sessionId, "claim")).toBe(
      `/api/guest-sessions/${sessionId}/claim`,
    );
    expect(guestSessionPath(sessionId, "revoke")).toBe(
      `/api/guest-sessions/${sessionId}/revoke`,
    );
    expect(() => guestSessionPath(`${sessionId}/../other`)).toThrow(
      "Invalid guest session id",
    );
  });

  it("parses guest auth and does not require legacy servers to return session metadata", () => {
    const parsed = GuestAuthResponseSchema.parse({
      token: guestToken,
      user: {
        id: "018f03a0-c4d2-7a37-ae4d-5aa45de12f12",
        name: "Guest",
        email: "guest@example.invalid",
      },
    });

    expect(parsed.token).toBe(guestToken);
    expect(parsed.session_id).toBeUndefined();
  });

  it("fails closed to a terminal state for an unknown session status", () => {
    const parsed = GuestSessionSchema.parse({
      id: sessionId,
      user_id: "018f03a0-c4d2-7a37-ae4d-5aa45de12f12",
      status: "future-status",
      created_at: "2026-09-02T00:00:00Z",
    });

    expect(parsed.status).toBe("revoked");
  });
});
