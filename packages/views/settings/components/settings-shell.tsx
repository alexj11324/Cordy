"use client";

/**
 * The layout shell every migrated settings tab is built from — the Lobe UI
 * replacements for `SettingsSection` / `SettingsCard` / `SettingsRow`.
 *
 * `SettingsGroup` is a section (title / description / action) that can also be
 * the card around its rows; `SettingsField` is one labelled row.
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
 * `Form` is imported from its own entry rather than `@lobehub/ui`'s root. The
 * root re-exports every component the package ships; importing it costs ~9.8s
 * of module load under Vitest against ~1.3s for this entry, and the package
 * publishes `./es/*` in its exports map for exactly this. Every migrated
 * settings tab renders through this module.
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

export interface SettingsFieldProps {
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

export function SettingsField({
  label,
  description,
  minWidth,
  divider,
  children,
  className,
}: SettingsFieldProps) {
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
