// @vitest-environment node
import { describe, expect, it } from "vitest";
import { findActiveHubInstallation } from "./dev-acceptance-provider";

describe("complete Electron provider acceptance", () => {
  it("does not mistake an agent-scoped installation for the workspace Hub", () => {
    expect(
      findActiveHubInstallation([
        { agent_id: "agent-1", status: "active", round_trip_status: "passed" },
      ]),
    ).toBeUndefined();
  });

  it("returns an active Hub even when its message round trip is still pending", () => {
    const installation = { agent_id: null, status: "active", round_trip_status: "not_run" };
    expect(findActiveHubInstallation([installation])).toBe(installation);
  });
});
