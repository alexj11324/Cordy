import * as SecureStore from "expo-secure-store";
import { chatActiveSessionStorageKey } from "@/lib/chat-session-state";

/**
 * Active chat selection is view state, not server data. SecureStore keeps it
 * across launches without adding a second mobile persistence dependency. A
 * storage failure must never block opening or sending in Chat.
 */
export async function loadChatActiveSession(
  workspaceId: string,
): Promise<string | null> {
  try {
    return await SecureStore.getItemAsync(chatActiveSessionStorageKey(workspaceId));
  } catch {
    return null;
  }
}

export async function saveChatActiveSession(
  workspaceId: string,
  sessionId: string | null,
): Promise<void> {
  try {
    const key = chatActiveSessionStorageKey(workspaceId);
    if (sessionId) {
      await SecureStore.setItemAsync(key, sessionId);
    } else {
      await SecureStore.deleteItemAsync(key);
    }
  } catch {
    // View-state persistence is best effort. Keep the chat usable if the
    // platform keychain is unavailable (e.g. a simulator without a keychain).
  }
}
