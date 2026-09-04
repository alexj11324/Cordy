"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import type { SettingsSaveStatus } from "./settings-layout";

interface UseAutoSaveOptions<T> {
  value: T;
  savedValue: T;
  onSave: (value: T) => Promise<void>;
  onSuccess?: (value: T) => void;
  onError?: (error: unknown) => void;
  enabled?: boolean;
  delay?: number;
  isEqual: (left: T, right: T) => boolean;
}

interface AutoSaveResult<T> {
  status: SettingsSaveStatus;
  /** Persist the latest draft now. Resolves true when that draft matches what
   *  last succeeded; false if a request failed or saving was disabled. */
  flush: () => Promise<boolean>;
  saveNow: (value: T) => void;
}

/**
 * Debounces text-heavy settings while serializing requests. If a user edits
 * again during an in-flight save, only the latest queued value is persisted
 * next, so a slower response can never overwrite a newer request.
 */
export function useAutoSave<T>({
  value,
  savedValue,
  onSave,
  onSuccess,
  onError,
  enabled = true,
  delay = 650,
  isEqual,
}: UseAutoSaveOptions<T>): AutoSaveResult<T> {
  const [status, setStatus] = useState<SettingsSaveStatus>("idle");
  const mountedRef = useRef(true);
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const savingRef = useRef(false);
  const queuedRef = useRef<T | null>(null);
  const inFlightRef = useRef<Promise<void> | null>(null);
  const latestValueRef = useRef(value);
  const persistedRef = useRef(savedValue);
  const observedSavedRef = useRef(savedValue);
  const enabledRef = useRef(enabled);
  const onSaveRef = useRef(onSave);
  const onSuccessRef = useRef(onSuccess);
  const onErrorRef = useRef(onError);
  const isEqualRef = useRef(isEqual);

  latestValueRef.current = value;
  enabledRef.current = enabled;
  onSaveRef.current = onSave;
  onSuccessRef.current = onSuccess;
  onErrorRef.current = onError;
  isEqualRef.current = isEqual;

  if (!isEqual(savedValue, observedSavedRef.current)) {
    observedSavedRef.current = savedValue;
    persistedRef.current = savedValue;
  }

  const runSave = useCallback(async (next: T) => {
    if (savingRef.current) {
      queuedRef.current = next;
      const inFlight = inFlightRef.current;
      if (inFlight) await inFlight;
      if (
        enabledRef.current &&
        !isEqualRef.current(next, persistedRef.current)
      ) {
        await runSave(next);
      }
      return;
    }

    if (!enabledRef.current || isEqualRef.current(next, persistedRef.current)) {
      return;
    }

    queuedRef.current = next;
    savingRef.current = true;
    let settleInFlight = () => {};
    const inFlight = new Promise<void>((resolve) => {
      settleInFlight = resolve;
    });
    inFlightRef.current = inFlight;

    let succeeded = false;
    let lastSaved: T | null = null;
    if (mountedRef.current) setStatus("saving");
    try {
      while (enabledRef.current) {
        const toSave = queuedRef.current;
        queuedRef.current = null;
        if (toSave == null || isEqualRef.current(toSave, persistedRef.current)) {
          break;
        }
        try {
          await onSaveRef.current(toSave);
          persistedRef.current = toSave;
          lastSaved = toSave;
          succeeded = true;
        } catch (error) {
          if (mountedRef.current) setStatus("error");
          onErrorRef.current?.(error);
          succeeded = false;
          break;
        }
      }
      if (succeeded && lastSaved != null && mountedRef.current) {
        setStatus("saved");
        onSuccessRef.current?.(lastSaved);
      }
    } finally {
      savingRef.current = false;
      inFlightRef.current = null;
      settleInFlight();
    }
  }, []);

  const saveNow = useCallback(
    (next: T) => {
      if (timerRef.current) {
        clearTimeout(timerRef.current);
        timerRef.current = null;
      }
      void runSave(next);
    },
    [runSave],
  );

  const flush = useCallback(async () => {
    if (timerRef.current) {
      clearTimeout(timerRef.current);
      timerRef.current = null;
    }
    await runSave(latestValueRef.current);
    return isEqualRef.current(latestValueRef.current, persistedRef.current);
  }, [runSave]);

  useEffect(() => {
    if (timerRef.current) clearTimeout(timerRef.current);

    if (!enabled || isEqual(value, persistedRef.current)) {
      timerRef.current = null;
      if (!enabled) setStatus("idle");
      return;
    }

    setStatus("saving");
    timerRef.current = setTimeout(() => {
      timerRef.current = null;
      void runSave(latestValueRef.current);
    }, delay);

    return () => {
      if (timerRef.current) {
        clearTimeout(timerRef.current);
        timerRef.current = null;
      }
    };
  }, [delay, enabled, isEqual, runSave, value]);

  useEffect(() => {
    // Restore the invariant on every mount. Under React StrictMode the initial
    // setup/cleanup/setup cycle would otherwise leave mountedRef pinned to false
    // for the component's whole life, stranding status at "saving".
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
      if (timerRef.current) clearTimeout(timerRef.current);
    };
  }, []);

  return { status, flush, saveNow };
}
