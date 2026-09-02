import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  deleteItemAsync,
  getItemAsync,
  setItemAsync,
} from "expo-secure-store";
import {
  loadChatActiveSession,
  saveChatActiveSession,
} from "./chat-session-storage";

vi.mock("expo-secure-store", () => ({
  deleteItemAsync: vi.fn(),
  getItemAsync: vi.fn(),
  setItemAsync: vi.fn(),
}));

describe("chat session storage", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("persists and restores a selection under the workspace-specific key", async () => {
    vi.mocked(getItemAsync).mockResolvedValue("session-2");

    await saveChatActiveSession("workspace-a", "session-2");
    const restored = await loadChatActiveSession("workspace-a");

    expect(setItemAsync).toHaveBeenCalledWith(
      "patchbay_chat_active_session_v1:workspace-a",
      "session-2",
    );
    expect(getItemAsync).toHaveBeenCalledWith(
      "patchbay_chat_active_session_v1:workspace-a",
    );
    expect(restored).toBe("session-2");
  });

  it("deletes the workspace selection when the native chat enters new-chat state", async () => {
    await saveChatActiveSession("workspace-a", null);

    expect(deleteItemAsync).toHaveBeenCalledWith(
      "patchbay_chat_active_session_v1:workspace-a",
    );
  });

  it("keeps chat usable when the platform keychain rejects an operation", async () => {
    vi.mocked(getItemAsync).mockRejectedValue(new Error("keychain unavailable"));
    vi.mocked(setItemAsync).mockRejectedValue(new Error("keychain unavailable"));
    vi.mocked(deleteItemAsync).mockRejectedValue(new Error("keychain unavailable"));

    await expect(loadChatActiveSession("workspace-a")).resolves.toBeNull();
    await expect(
      saveChatActiveSession("workspace-a", "session-1"),
    ).resolves.toBeUndefined();
    await expect(
      saveChatActiveSession("workspace-a", null),
    ).resolves.toBeUndefined();
  });
});
