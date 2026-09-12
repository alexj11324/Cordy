"use client";

/**
 * The settings page's single-choice select.
 *
 * **Why the root `Select` and not `@lobehub/ui/base-ui`'s.** The base-ui
 * `Select` is the one the library points at (`@lobehub/ui`'s root `Select`
 * carries an `@deprecated` tag saying to use it instead), and it uses a fixed
 * prop list with no rest spread — exactly like the base-ui `Switch`. Its
 * trigger is a `<button role="combobox">`, and no prop we can pass reaches that
 * button, so the control cannot be given an accessible name: `aria-label` is
 * dropped, and wrapping it in a base-ui `Field.Root` does not help either (the
 * generated `<label for>` points at the root's hidden input, while the trigger
 * keeps a different generated id and no `aria-labelledby` — verified against
 * the installed package, not inferred).
 *
 * The root `Select` is antd-backed, consumes the antd theme the settings
 * bridge installs, and forwards `aria-label` to the `<input role="combobox">`
 * it renders. The trade is a deprecation notice against a control that a screen
 * reader can actually announce.
 *
 * The wrapper exists so a call site cannot forget that trade: `label` is
 * required, and it is the only prop that maps to the accessible name. Same
 * reasoning as `SettingsGroup` passing `collapsible={false}` — one silent
 * default made impossible in one place.
 *
 * `value` is a plain string rather than a generic. antd's option and change
 * types are wide (`string | number | null`), and narrowing them back to a
 * literal union inside the wrapper buys nothing the call site cannot do with
 * one cast at the boundary where it already knows the union.
 *
 * `label` is a plain string for the same reason: antd copies it into the
 * option's `title` attribute, and that attribute is the only handle a test can
 * take on an option. The popup's items are plain `div`s — antd v6 exposes the
 * options to assistive technology through a separate, clipped `[role=option]`
 * list whose nodes are not the ones a pointer hits — so `role` queries can
 * assert that an option exists but cannot click it. A ReactNode label leaves
 * `title` unset and takes that handle away. Styling that used to ride on the
 * option node therefore belongs on `className` (the trigger) and
 * `popupClassName` (the list).
 */

import Select from "@lobehub/ui/es/Select/Select";

export interface SettingsSelectOption {
  /** Plain text — see the module note on why this is not a `ReactNode`. */
  value: string;
  label: string;
}

export interface SettingsSelectProps {
  /** Accessible name for the trigger. Required — see the module note. */
  label: string;
  value: string;
  options: readonly SettingsSelectOption[];
  onValueChange: (value: string) => void;
  disabled?: boolean;
  id?: string;
  className?: string;
  /** Extra class for the popup root, for styling the option list itself. */
  popupClassName?: string;
}

export function SettingsSelect({
  label,
  value,
  options,
  onValueChange,
  disabled,
  id,
  className,
  popupClassName,
}: SettingsSelectProps) {
  return (
    <Select
      aria-label={label}
      className={className}
      disabled={disabled}
      id={id}
      options={options.map((option) => ({
        value: option.value,
        label: option.label,
      }))}
      popupClassName={popupClassName}
      value={value}
      onChange={(next) => onValueChange(String(next))}
    />
  );
}
