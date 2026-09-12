"use client";

/**
 * The settings page's search box — Lobe's `SearchBar`, deep-imported.
 *
 * Its own module rather than another export from `settings-shell`, for the same
 * fan-out reason `settings-empty.tsx` gives: a module's graph is paid once per
 * test file that imports it, and `settings-shell` is imported by every migrated
 * tab, so a control only the catalog tabs render would be charged to the tabs
 * that never render it.
 *
 * The deep path is not a preference. `SearchBar` is reachable from the root
 * `@lobehub/ui` barrel only — `es/base-ui/index.d.mts` does not list it, while
 * it does list `Input`, `Modal`, `Button`, `ActionIcon` and `DropdownMenu` — and
 * the root barrel is the ~11s import this directory's notes measure. Deep
 * importing pays for `SearchBar`'s own graph and nothing else.
 *
 * `label` is required and is the accessible name, the same
 * one-silent-default discipline as `SettingsSwitch` and `SettingsSelect`. Unlike
 * those two there is nothing to work around to make it land: `SearchBar`
 * renders antd's `Input` and hands it `...rest` **after** its own handlers
 * (`es/SearchBar/SearchBar.mjs`, the `jsx(Input, { … onPressEnter …, ...rest })`
 * call), so `aria-label` and `id` reach the real `<input>`. The three
 * accessibility traps this migration hit — base-ui `Switch` and `Select`
 * dropping `aria-label`, `role` filtered out of `Form.Group` — are all cases
 * where a prop stops at a container; this is the case where it does not.
 *
 * `SearchBar` also renders a keyboard-shortcut badge and a spotlight overlay,
 * both off unless asked for, and turns on `allowClear`, which the old
 * `SettingsSearchField` had to leave to the caller.
 */

import SearchBar from "@lobehub/ui/es/SearchBar/SearchBar";
import type { CSSProperties } from "react";

export interface SettingsSearchBarProps {
  /** Accessible name for the input. Required — see the module note. */
  label: string;
  value: string;
  onValueChange: (value: string) => void;
  placeholder: string;
  id?: string;
  /** Layout class for the wrapper. */
  className?: string;
  style?: CSSProperties;
}

export function SettingsSearchBar({
  label,
  value,
  onValueChange,
  placeholder,
  id,
  className,
  style,
}: SettingsSearchBarProps) {
  return (
    <SearchBar
      aria-label={label}
      className={className}
      id={id}
      placeholder={placeholder}
      style={style}
      value={value}
      onInputChange={onValueChange}
    />
  );
}
