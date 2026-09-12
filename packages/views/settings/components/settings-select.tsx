"use client";

/**
 * The settings page's single-choice select.
 *
 * **Why the antd-backed root `Select` and not `@lobehub/ui/base-ui`'s.** The
 * base-ui component is the one the library points at, but it gives its trigger
 * no way to be named: it destructures a fixed prop list and spreads nothing, so
 * `aria-label` never reaches the `<button role="combobox">`. Two dead ends,
 * measured, so nobody re-measures them: a visually-hidden label in the
 * `prefix` slot does reach the DOM but produces an empty accessible name,
 * because `combobox` does not allow name-from-content; and a base-ui
 * `Field.Root` is not available here at all, since Lobe's base-ui barrel
 * exports neither `Field` nor `SelectLabel` — importing `@base-ui/react/field`
 * directly would break the "declare directly imported external packages" rule.
 *
 * **A third path does exist and is deliberately not taken.** The trigger *atom*
 * (`SelectTrigger`) does forward `aria-label` and does produce a real
 * accessible name. What it costs is everything the `Select` component supplies
 * on top of the atoms — search, virtua virtualization, value rendering — which
 * the ~600-item IANA timezone list needs. So the trade is not "deprecated vs
 * current": it is "a deprecated wrapper that can be named" against "atoms that
 * can be named but that we would have to re-implement". If a future surface
 * needs a nameable select over a *short* list, the atoms are the right answer
 * there.
 *
 * The wrapper exists so a call site cannot forget the trade: `label` is
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
 *
 * `popupClassName` is translated rather than forwarded. antd renamed the prop
 * to `classNames.popup.root` and warns on the old name
 * (`antd/es/select/index.js`, its `deprecatedProps` table); forwarding it would
 * put that warning in the console of every app that renders a settings select,
 * and the call sites would each have to know the new spelling. One line here
 * means the next six tabs cannot reintroduce it.
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
      classNames={
        popupClassName ? { popup: { root: popupClassName } } : undefined
      }
      disabled={disabled}
      id={id}
      options={options.map((option) => ({
        value: option.value,
        label: option.label,
      }))}
      value={value}
      onChange={(next) => onValueChange(String(next))}
    />
  );
}
