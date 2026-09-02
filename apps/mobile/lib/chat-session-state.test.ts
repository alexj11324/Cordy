import { describe, expect, it } from "vitest";
import {
  chatActiveSessionStorageKey,
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
});
