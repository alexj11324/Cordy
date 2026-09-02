import * as SecureStore from "expo-secure-store";

import { isGuestToken } from "./guest-auth";

const GUEST_CREDENTIALS_KEY = "patchbay_guest_credentials";

export type StoredGuestCredentials = {
  token: string;
  sessionId: string | null;
};

export async function saveGuestCredentials(
  token: string,
  sessionId?: string,
): Promise<void> {
  if (!isGuestToken(token)) {
    throw new Error("Invalid guest token");
  }
  await SecureStore.setItemAsync(
    GUEST_CREDENTIALS_KEY,
    JSON.stringify({ token, sessionId: sessionId ?? null }),
  );
}

export async function getGuestCredentials(): Promise<StoredGuestCredentials | null> {
  const raw = await SecureStore.getItemAsync(GUEST_CREDENTIALS_KEY);
  if (!raw) return null;

  try {
    const value: unknown = JSON.parse(raw);
    if (typeof value !== "object" || value === null) return null;
    const record = value as Record<string, unknown>;
    if (!isGuestToken(record.token)) return null;
    if (
      record.sessionId !== null &&
      record.sessionId !== undefined &&
      typeof record.sessionId !== "string"
    ) {
      return null;
    }
    return {
      token: record.token,
      sessionId: record.sessionId ?? null,
    };
  } catch {
    return null;
  }
}

export async function clearGuestCredentials(): Promise<void> {
  await SecureStore.deleteItemAsync(GUEST_CREDENTIALS_KEY);
}
