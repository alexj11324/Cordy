"use client";

/**
 * The settings page's empty state — Lobe's `Empty`, deep-imported.
 *
 * `keyboard-shortcuts-tab` used to reach `Empty` through the `@lobehub/ui` root
 * barrel, which was the only root-barrel import in this directory. Measured per
 * test file, the import phase alone is ~11.3s through the barrel against ~4.9s
 * for `@lobehub/ui/base-ui` and ~1.3s for a deep path — paid by every suite
 * that renders the tab, for a component used on one "no results" state.
 *
 * Its own module rather than another export from `settings-shell`: a module's
 * graph is paid once per test file that imports it, and `settings-shell` is
 * imported by every migrated tab, so `Empty` there would be charged to tabs
 * that never render it. That is the same fan-out reasoning the shell's own note
 * gives for deep-importing `Form`.
 */

import Empty from "@lobehub/ui/es/Empty/Empty";
import type { ReactNode } from "react";

export interface SettingsEmptyStateProps {
  title: ReactNode;
  description?: ReactNode;
  /**
   * Semantic tone for the heading. `"danger"` exists because **a failed request
   * and an empty collection are different states**, and folding one into the
   * other makes them distinguishable only by reading the sentence.
   *
   * The tone is not invented here: Lobe's `Empty` forwards `titleProps` to its
   * own `Text`, whose `type` is a semantic enum
   * (`'secondary' | 'success' | 'warning' | 'danger' | 'info'`) — verified in
   * the destructure list of `es/Text/Text.mjs`, not only in its type. This prop
   * is the one thing a call site cannot get wrong by forgetting it, because the
   * default is the neutral treatment every existing consumer already has.
   */
  tone?: "default" | "danger";
  className?: string;
}

/**
 * `SettingsEmptyState`, not `SettingsEmpty`: that name still belongs to
 * `settings-layout`'s atom until its last consumer is migrated, and a barrel
 * cannot export two different components under one name. Same reason the
 * shell's row is `SettingsFormRow`.
 */
export function SettingsEmptyState({
  title,
  description,
  tone = "default",
  className,
}: SettingsEmptyStateProps) {
  return (
    <Empty
      className={className}
      description={description}
      title={title}
      titleProps={tone === "danger" ? { type: "danger" } : undefined}
    />
  );
}
