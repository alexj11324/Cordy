/**
 * Thin wrapper around expo-secure-store for the auth token.
 * Keyed identically to web/desktop ("orvilo_token") so logic stays aligned
 * with packages/core/auth/store.ts even though storage backends differ.
 */
import * as SecureStore from "expo-secure-store";

const TOKEN_KEY = "orvilo_token";
const LEGACY_GUEST_CREDENTIALS_KEY = "orvilo_guest_credentials";

export async function getToken(): Promise<string | null> {
  return SecureStore.getItemAsync(TOKEN_KEY);
}

export async function setToken(token: string): Promise<void> {
  await SecureStore.setItemAsync(TOKEN_KEY, token);
}

export async function clearToken(): Promise<void> {
  await SecureStore.deleteItemAsync(TOKEN_KEY);
}

/** Remove credentials written by the short-lived mobile Guest experiment. */
export async function clearLegacyGuestCredentials(): Promise<void> {
  await SecureStore.deleteItemAsync(LEGACY_GUEST_CREDENTIALS_KEY);
}
