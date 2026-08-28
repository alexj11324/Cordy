import type { StorageAdapter } from "../types/storage";

const LEGACY_BRAND_KEY = /cordy/gi; // legacy-brand-compat

function migrateStorageKeys(storage: Storage): void {
  try {
    const keys = Array.from({ length: storage.length }, (_, index) =>
      storage.key(index),
    ).filter((key): key is string => key !== null);
    for (const oldKey of keys) {
      const newKey = oldKey.replace(LEGACY_BRAND_KEY, "patchbay");
      if (newKey === oldKey) continue;
      if (storage.getItem(newKey) === null) {
        const value = storage.getItem(oldKey);
        if (value !== null) storage.setItem(newKey, value);
      }
      storage.removeItem(oldKey);
    }
  } catch {
    // Storage can be disabled by browser policy; the adapter remains best-effort.
  }
}

if (typeof window !== "undefined") {
  migrateStorageKeys(window.localStorage);
  migrateStorageKeys(window.sessionStorage);
}

/** SSR-safe localStorage. Works in both Next.js (SSR) and Electron (always client). */
export const defaultStorage: StorageAdapter = {
  getItem: (k) =>
    typeof window !== "undefined" ? localStorage.getItem(k) : null,
  setItem: (k, v) => {
    if (typeof window !== "undefined") localStorage.setItem(k, v);
  },
  removeItem: (k) => {
    if (typeof window !== "undefined") localStorage.removeItem(k);
  },
};
