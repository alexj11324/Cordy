import { describe, expect, it } from "vitest";
import { AUTH_CONTRACT } from "./contract";

describe("auth broker session contract", () => {
  it("names the Go session, guest, logout, API, and WS boundaries", () => {
    expect(AUTH_CONTRACT.client).toEqual({
      clerkExchangePath: "/auth/clerk",
      guestPath: "/auth/guest",
      logoutPath: "/auth/logout",
      mePath: "/api/me",
      websocketPath: "/ws",
      guestTokenPrefix: "pbg_",
      guestWorkspaceAccess: false,
      guestWebsocketAccess: false,
    });
  });
});
