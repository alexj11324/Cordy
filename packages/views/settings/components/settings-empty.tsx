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
  className,
}: SettingsEmptyStateProps) {
  return (
    <Empty className={className} description={description} title={title} />
  );
}
