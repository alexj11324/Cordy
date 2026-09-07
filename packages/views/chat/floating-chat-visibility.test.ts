// @vitest-environment node
import { describe, expect, it } from "vitest";
import { isFloatingChatRouteSuppressed } from "./floating-chat-visibility";

describe("floating chat route suppression", () => {
  it("suppresses the overlay on the Chat tab and its sub-routes", () => {
    expect(isFloatingChatRouteSuppressed("/acme/chat", "/acme/chat")).toBe(
      true,
    );
    expect(
      isFloatingChatRouteSuppressed("/acme/chat/session-1", "/acme/chat"),
    ).toBe(true);
  });

  it("keeps the overlay on every other route, including prefix look-alikes", () => {
    expect(isFloatingChatRouteSuppressed("/acme/issues", "/acme/chat")).toBe(
      false,
    );
    // A sibling route that merely starts with the same characters is not the
    // Chat tab — matching on the bare prefix would hide chat from it.
    expect(isFloatingChatRouteSuppressed("/acme/chatter", "/acme/chat")).toBe(
      false,
    );
    // Another workspace's chat tab is a different route too.
    expect(isFloatingChatRouteSuppressed("/other/chat", "/acme/chat")).toBe(
      false,
    );
  });

  it("suppresses the overlay on an agent detail route but keeps it on the agents list", () => {
    expect(
      isFloatingChatRouteSuppressed(
        "/acme/agents/agent-1",
        "/acme/chat",
        undefined,
        "/acme/agents",
        true,
      ),
    ).toBe(true);
    expect(
      isFloatingChatRouteSuppressed(
        "/acme/agents",
        "/acme/chat",
        undefined,
        "/acme/agents",
      ),
    ).toBe(false);
    expect(
      isFloatingChatRouteSuppressed(
        "/acme/agents/new/manual",
        "/acme/chat",
        undefined,
        "/acme/agents",
      ),
    ).toBe(false);
    expect(
      isFloatingChatRouteSuppressed(
        "/acme/agents-old/agent-1",
        "/acme/chat",
        undefined,
        "/acme/agents",
      ),
    ).toBe(false);
  });

  it("keeps global chat when Agent detail has no working replacement DM action", () => {
    expect(
      isFloatingChatRouteSuppressed(
        "/acme/agents/agent-1",
        "/acme/chat",
        undefined,
        "/acme/agents",
        false,
      ),
    ).toBe(false);
  });

  it("keeps the launcher out of an active Agent composer and restores it on other routes", () => {
    expect(
      isFloatingChatRouteSuppressed(
        "/acme/automations/one",
        "/acme/chat",
        "/acme/automations/one",
      ),
    ).toBe(true);
    expect(
      isFloatingChatRouteSuppressed(
        "/acme/issues",
        "/acme/chat",
        "/acme/automations/one",
      ),
    ).toBe(false);
    expect(
      isFloatingChatRouteSuppressed("/acme/automations/one", "/acme/chat"),
    ).toBe(false);
  });
});
