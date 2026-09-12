"use client";

/**
 * The settings page's confirmation dialog, replacing the 22 `AlertDialog`
 * call sites one tab at a time.
 *
 * `confirmModal` is imperative and takes its copy as plain props, which makes
 * three things easy to get wrong once each. This wrapper is the one place they
 * are got right:
 *
 * - **Destructive tone.** `ModalConfirmConfig` has no `danger` or `tone`
 *   field; `okButtonProps` is the only way in, and it is spread onto
 *   `<Button type="primary" ...>`, so `danger` is what produces the red solid
 *   treatment `AlertDialogAction` used to have.
 *
 * - **Localized buttons.** `confirmModal` defaults to English (`"Cancel"`,
 *   `"OK"`) with no `ConfigProvider` locale behind it, so a call site that
 *   forgets the labels shows English exactly when the user is about to lose
 *   something. Reading them from `useT("settings")` here means a call site
 *   cannot forget.
 *
 * - **Closing only after the server answers.** `confirmModal`'s close logic is
 *   `try { const result = onOk(); if (thenable) await result } catch { return }
 *   close()`: the dialog dismisses on the *next line* unless `onOk` returns a
 *   promise, and stays open when that promise rejects. A call site that writes
 *   `onOk: () => { void mutate() }` closes immediately and, when the request
 *   then fails, leaves the UI claiming a row is gone that never went — which
 *   the project's state rules forbid on destructive flows. So `onConfirm` is
 *   typed to return `Promise<void>`, and a non-promise return at runtime is
 *   turned into a throw, which the library catches as "keep the dialog open".
 *   Rejections from `onConfirm` propagate untouched — do not catch inside it;
 *   surface the failure in the caller (a toast) and rethrow.
 */

import { confirmModal, type ModalConfirmConfig } from "@lobehub/ui/base-ui";
import { useCallback } from "react";
import type { ReactNode } from "react";

import { useT } from "../../i18n";

export interface SettingsConfirmOptions {
  title: ReactNode;
  description?: ReactNode;
  /** Verb on the confirming button, already translated by the caller. */
  confirmLabel?: ReactNode;
  cancelLabel?: ReactNode;
  /**
   * Defaults to `destructive`: a confirm that turns out to be harmless looks
   * alarmist for one render, while a destructive confirm that forgot the tone
   * reads as safe. Pass `"default"` for the genuinely non-destructive ones.
   */
  tone?: "default" | "destructive";
  /**
   * The request to run before the dialog closes. Must return its promise and
   * must reject when the request fails.
   */
  onConfirm: () => Promise<void>;
}

export interface SettingsConfirmHandle {
  close: () => void;
  destroy: () => void;
}

class MissingPromiseError extends Error {
  constructor() {
    super(
      "useSettingsConfirm: `onConfirm` must return the promise for its request, or the dialog closes before the server answers.",
    );
  }
}

function isThenable(value: unknown): value is PromiseLike<void> {
  return (
    typeof value === "object" &&
    value !== null &&
    typeof (value as PromiseLike<void>).then === "function"
  );
}

/**
 * Returns the confirm trigger for the current locale. Call sites use this
 * rather than importing `confirmModal` directly so the contract above holds
 * for every dialog on the page.
 */
export function useSettingsConfirm(): (
  options: SettingsConfirmOptions,
) => SettingsConfirmHandle {
  const { t } = useT("settings");

  return useCallback(
    ({
      title,
      description,
      confirmLabel,
      cancelLabel,
      tone = "destructive",
      onConfirm,
    }: SettingsConfirmOptions) => {
      const config: ModalConfirmConfig = {
        title,
        content: description,
        okText: confirmLabel ?? t(($) => $.confirm.confirm),
        cancelText: cancelLabel ?? t(($) => $.confirm.cancel),
        okButtonProps: { danger: tone === "destructive" },
        onOk: () => {
          const result = onConfirm();
          if (!isThenable(result)) throw new MissingPromiseError();
          return result;
        },
      };
      return confirmModal(config);
    },
    [t],
  );
}
