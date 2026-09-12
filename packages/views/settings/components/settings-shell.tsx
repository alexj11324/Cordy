"use client";

/**
 * The layout shell every migrated settings tab is built from — the Lobe UI
 * replacements for `SettingsSection` / `SettingsCard` / `SettingsRow`.
 *
 * `SettingsGroup` is a section (title / description / action) that can also be
 * the card around its rows; `SettingsFormRow` is one labelled row.
 *
 * The row is `SettingsFormRow` rather than `SettingsRow` or `SettingsField`
 * because both of those names are still exported, for a while longer, by
 * `settings-layout` — and a barrel cannot export two different components under
 * one name. It is semantically the old `SettingsRow`: the `Form.Item` that owns
 * a row's label, description and control slot.
 *
 * `SettingsGroup` exists mostly to make one silent default impossible:
 * `Form.Group` derives collapsibility from the variant when `collapsible` is
 * left undefined (`collapsible === undefined ? !isBorderless : collapsible`),
 * so an outlined group grows a disclosure caret and every settings section
 * becomes an accordion — which also breaks the page's nav search, because that
 * matches on row content and a collapsed section matches nothing. Passing
 * `collapsible={false}` here means no call site has to know that.
 *
 * Neither component takes a `name`. antd's `Form` is uncontrolled, and the
 * settings fields are not: toggles write straight to the store and drafts
 * belong to `useAutoSave`. `Form.Item` is here for layout only. See
 * `reference-lobe-tab-migration.md` on state ownership before giving a field a
 * `name`.
 *
 * `Form` is imported from its own entry rather than `@lobehub/ui`'s root. What
 * makes an import like this worth an exception is **fan-out, not layer**: a
 * module's graph is paid once per test file that imports it, so the question is
 * how many suites sit behind the import, not which layer the file belongs to.
 * Every migrated tab renders through this module, so its cost multiplies by the
 * whole tab group exactly the way the theme bridge's does. Measured warm, three
 * runs each, in this worktree: the root entry costs 8.88s / 8.10s / 8.84s per
 * file (7.6-8.4s of that import) against 1.30s / 1.28s / 1.27s for this one.
 * The package declares `"./es/*"` in its exports map, so the deep path is a
 * supported subpath, and it resolves to the same module the root re-exports —
 * confirmed by execution, not assumed. That check is not kept here: importing
 * the root to compare against costs 18s in the suite that renders this module,
 * which is the fan-out cost re-created inside a test. What
 * `lobe/lobe-theme-bridge.test.tsx` asserts cheaply is the weaker but still
 * load-bearing half: this package resolves to a *single instance* across a
 * barrel and a deep path (`expect(DeepModalHost).toBe(BarrelModalHost)`). It
 * compares a different module, so it does not by itself prove the two `Form`
 * specifiers agree; it does prove the resolution guarantee they both rely on.
 *
 * The exception stops here. A tab importing a Lobe component directly uses the
 * barrel — the point of this shell is that tabs mostly will not need to.
 */

import Form from "@lobehub/ui/es/Form/index";
import type { ReactNode } from "react";

export interface SettingsGroupProps {
  title?: ReactNode;
  description?: ReactNode;
  /** Trailing slot on the header row — the old `SettingsSection`'s `action`. */
  extra?: ReactNode;
  /**
   * `outlined` draws the bordered card `SettingsCard` used to be; `borderless`
   * is a plain section for rows that supply their own chrome.
   */
  variant?: "outlined" | "borderless";
  children: ReactNode;
  className?: string;
}

export function SettingsGroup({
  title,
  description,
  extra,
  variant = "outlined",
  children,
  className,
}: SettingsGroupProps) {
  return (
    <Form.Group
      className={className}
      collapsible={false}
      desc={description}
      extra={extra}
      title={title}
      variant={variant}
    >
      {children}
    </Form.Group>
  );
}

export interface SettingsFormRowProps {
  label?: ReactNode;
  description?: ReactNode;
  /**
   * Width floor for the control column, in pixels. The old `SettingsRow` used
   * five container-query tiers; `Form.Item` sizes from the value itself.
   */
  minWidth?: number;
  /** Draw a separator above this row. */
  divider?: boolean;
  children: ReactNode;
  className?: string;
}

export function SettingsFormRow({
  label,
  description,
  minWidth,
  divider,
  children,
  className,
}: SettingsFormRowProps) {
  return (
    <Form.Item
      className={className}
      desc={description}
      divider={divider}
      label={label}
      minWidth={minWidth}
    >
      {children}
    </Form.Item>
  );
}
