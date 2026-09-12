"use client";

/**
 * Puts `@lobehub/ui`'s imperative-modal host in the tree — from exactly one
 * live `LobeThemeBridge`.
 *
 * `confirmModal` / `createModal` push onto a module-level stack that nothing in
 * the package renders: `@lobehub/ui` ships the host as a component the consumer
 * has to mount. Without it they no-op *silently* — no dialog, no error. (The
 * library does mount its toast host itself, which is why `toast()` works out of
 * the box and this does not.)
 *
 * The host cannot simply be rendered by every bridge, though. `@lobehub/ui`
 * guards it as a dev-mode singleton: `utils/devSingleton` throws
 * `[lobe-ui] BaseModalHost must be rendered only once in a single React tree`
 * on the second host in the same document. `LobeThemeBridge` is mounted per
 * surface — the chat message list, the feedback dialog, the settings dialog —
 * and those genuinely coexist (the feedback dialog opens over the chat page),
 * so duplicates are the normal case here, not an edge case. Because that guard
 * is `process.env.NODE_ENV === "development"` only, the failure mode is
 * production staying green while every dev build dies the moment someone opens
 * feedback over chat.
 *
 * So this slot elects one owner among the live bridges and re-elects when that
 * owner unmounts, keeping exactly one host mounted for as long as any bridge
 * is. A single bridge — the common case, and everything in tests — behaves
 * exactly as if it rendered `<ModalHost />` directly.
 *
 * `ModalHost` comes from the `@lobehub/ui/base-ui` barrel — the shallowest
 * entry the package publishes for it, and the only one that does not drag in
 * the package root, which measures ~9.8s of module graph under Vitest against
 * ~1.3s for a single component entry.
 */

import { ModalHost } from "@lobehub/ui/base-ui";
import { useEffect, useState, useSyncExternalStore } from "react";

/** The bridge that currently renders the host, or `null` when none does. */
let owner: symbol | null = null;

const listeners = new Set<() => void>();

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

function getOwner(): symbol | null {
  return owner;
}

function notify(): void {
  for (const listener of listeners) listener();
}

function claim(id: symbol): void {
  if (owner !== null) return;
  owner = id;
  notify();
}

function release(id: symbol): void {
  if (owner !== id) return;
  owner = null;
  notify();
}

export function LobeModalHost() {
  const [id] = useState(() => Symbol("lobe-modal-host"));
  const currentOwner = useSyncExternalStore(subscribe, getOwner, getOwner);

  // The claim is gated on module state rather than on `currentOwner`, and it
  // re-runs whenever a different bridge takes over. Reading the snapshot here
  // instead would strand the slot: StrictMode runs this effect, cleans it up,
  // then runs it again, and by the second run the snapshot still names us even
  // though the cleanup already gave the claim back.
  useEffect(() => {
    claim(id);
  }, [currentOwner, id]);

  useEffect(() => () => release(id), [id]);

  return currentOwner === id ? <ModalHost /> : null;
}
