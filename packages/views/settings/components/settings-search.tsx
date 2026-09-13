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
 * those two there is nothing to work around to make it land — but it is a
 * **four-hop chain**, and the hop that would drop it is the last one, so it is
 * worth naming rather than assuming:
 *
 * 1. `es/SearchBar/SearchBar.mjs` hands `...rest` to Lobe's `Input` **after** its
 *    own handlers (`jsx(Input, { … onPressEnter …, ...rest })`), so a call
 *    site's prop is not overwritten by `onChange`/`onFocus`.
 * 2. `es/Input/Input.mjs` destructures `{ ref, variant, shadow, className,
 *    ...rest }` and spreads `...rest` into antd's `Input`.
 * 3. `antd/es/input/Input.js` spreads `...rest` into `@rc-component/input`
 *    (`import RcInput from '@rc-component/input'`, line 4).
 * 4. `@rc-component/input/es/Input.js` builds `otherProps = omit(props, [...])`
 *    and puts it on the real `<input>`. **This list is the one that matters** —
 *    a prop added to it would stop the chain here without any error — and
 *    `aria-label` and `id` are both absent from it.
 *
 * **Hop 4 is `@rc-component/input`, not `rc-input`.** Both exist in the store
 * and the names are near-identical, but `rc-input` is reached only through
 * Lobe's `rc-input-number` and is not on this chain. Verified by reading the
 * import in hop 3 rather than by matching on the name — the earlier version of
 * this comment cited `rc-input` and sent a reader to a file that never runs.
 *
 * Measured end to end rather than inferred: the renderer's `<input>` carries
 * `aria-label`, and `getByRole("textbox", { name })` resolves to exactly one
 * node. That check is pinned in the settings smoke script ("the catalog search
 * box carries an accessible name"), which fails when the prop is removed. The
 * script currently lives in this plan's workspace rather than the repo; Task 10
 * promotes it.
 *
 * The three accessibility traps this migration hit — base-ui `Switch` and
 * `Select` dropping `aria-label`, `role` filtered out of `Form.Group` — are all
 * cases where a prop stops at a container; this is the case where it does not.
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
