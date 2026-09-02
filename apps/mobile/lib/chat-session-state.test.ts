import { describe, expect, it } from "vitest";
import {
  chatActiveSessionStorageKey,
  chatAgentHref,
  chatRouteParams,
  firstChatRouteParam,
  resolveActiveChatSessionId,
} from "./chat-session-state";

describe("chat session state", () => {
  it("keeps the persisted session only when it belongs to the workspace list", () => {
    const sessions = [{ id: "newest" }, { id: "saved" }];

    expect(resolveActiveChatSessionId("saved", sessions)).toBe("saved");
    expect(resolveActiveChatSessionId("deleted", sessions)).toBe("newest");
  });

  it("keeps new-chat state when the workspace has no sessions", () => {
    expect(resolveActiveChatSessionId("deleted", [])).toBeNull();
    expect(resolveActiveChatSessionId(null, [])).toBeNull();
  });

  it("names storage by workspace so selections cannot cross workspaces", () => {
    expect(chatActiveSessionStorageKey("workspace-a")).toBe(
      "patchbay_chat_active_session_v1:workspace-a",
    );
    expect(chatActiveSessionStorageKey("workspace-a")).not.toBe(
      chatActiveSessionStorageKey("workspace-b"),
    );
  });

  it("normalizes Expo Router params without treating an empty array as an id", () => {
    expect(firstChatRouteParam("session-1")).toBe("session-1");
    expect(firstChatRouteParam(["session-1", "session-2"])).toBe("session-1");
    expect(firstChatRouteParam([])).toBeNull();
    expect(firstChatRouteParam(undefined)).toBeNull();
  });

  it("keeps the session route canonical and clears the alternate agent intent", () => {
    expect(chatRouteParams("session-1", "agent-1")).toEqual({
      session: "session-1",
      agent: undefined,
    });
    expect(chatRouteParams(null, "agent-1")).toEqual({
      session: undefined,
      agent: "agent-1",
    });
    expect(chatRouteParams(null, null)).toEqual({
      session: undefined,
      agent: undefined,
    });
  });

  it("builds the native Agent-to-Chat deep link with an encoded agent id", () => {
    expect(chatAgentHref("acme", "agent/one")).toBe(
      "/acme/chat?agent=agent%2Fone",
    );
  });
});
