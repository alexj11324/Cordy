import { describe, expect, it } from "vitest";
import {
  desktopSessionCompleteUrl,
  loopbackFreshKey,
  readLoopbackSessionApi,
} from "./session-api";

describe("readLoopbackSessionApi", () => {
  it("accepts the local product API origin", () => {
    expect(readLoopbackSessionApi("http://localhost:8080/")).toBe(
      "http://localhost:8080",
    );
    expect(readLoopbackSessionApi("http://127.0.0.1:19080")).toBe(
      "http://127.0.0.1:19080",
    );
  });

  it("rejects hosted, remote, and mixed-content targets", () => {
    expect(readLoopbackSessionApi("https://api.aspectlylabs.com")).toBeNull();
    expect(readLoopbackSessionApi("https://evil.example")).toBeNull();
    expect(readLoopbackSessionApi("http://evil.example")).toBeNull();
    expect(readLoopbackSessionApi("https://localhost:8080")).toBeNull();
    expect(readLoopbackSessionApi("http://localhost:8080/steal")).toBeNull();
    expect(
      readLoopbackSessionApi("http://localhost:8080/?next=https://evil.example"),
    ).toBeNull();
  });
});

describe("loopbackFreshKey", () => {
  it("scopes the fresh-sign-in flag to the desktop state", () => {
    expect(loopbackFreshKey("abc")).toBe("patchbay_desktop_loopback_fresh:abc");
  });
});

describe("desktopSessionCompleteUrl", () => {
  it("posts the Clerk session to the loopback complete path", () => {
    expect(desktopSessionCompleteUrl("http://localhost:8080")).toBe(
      "http://localhost:8080/auth/desktop-session/complete",
    );
  });
});
