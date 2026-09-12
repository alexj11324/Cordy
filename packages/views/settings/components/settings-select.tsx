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
 * option node therefore has to go through `typography` — see the next note for
 * why a class cannot carry it.
 *
 * **Type is `typography`, and it is inline style, because a class cannot win.**
 * This is the whole reason the prop is not called `className`. antd renamed the
 * old `popupClassName` to `classNames.popup.root` and warns on the old spelling
 * (`antd/es/select/index.js`, its `deprecatedProps` table), so the first version
 * of this wrapper translated the class to exactly that — and it did nothing:
 * `root` is the *container*, and each option is its own `div` in the virtualized
 * list. Moving the class to the option's own slot (`classNames.popup.listItem`,
 * which rc-select merges into the option's `clsx` at
 * `@rc-component/select/es/OptionList.js`) landed it on the right element and
 * *still* did not apply the size: antd's own rule for the option is
 * `:where(.css-x).ant-select-dropdown .ant-select-item-option` — 0-2-0 once
 * `:where()` contributes nothing — against the utility's 0-1-0. The trigger
 * loses the same way at a tie, since `:where(.css-x).ant-select` and
 * `.text-caption` are both 0-1-0 and antd's sheet is injected later.
 *
 * Measured on the timezone list with the classes correctly present, before this
 * prop existed: options 14px "Geist Mono Variable" (`font-family` is uncontested
 * on an option, so the *face* had applied and only the *size* had not), trigger
 * 14px "Inter Variable". The pre-migration design was 12px monospace in both.
 * This is not a difference a screenshot adjudicates, which is why the first
 * attempt shipped a popup screenshot that showed nothing.
 *
 * `styles` rather than `classNames` is therefore a measurement, not a
 * preference. antd turns the values into inline style — the option's `style` is
 * `...contextStyles?.popup?.listItem` on the same element antd styles, and the
 * trigger's is `style: mergedStyles.root` — and inline style outranks both
 * competing rules regardless of specificity or sheet order, with no
 * `!important` to maintain. The trigger's selected value follows for free: its
 * `.ant-select-content` has no `font-size` rule of its own, so it inherits.
 *
 * The values are still tokens (`var(--text-caption)`, `var(--font-mono)`), so a
 * call site names the design scale, not a pixel count. What it cannot do is use
 * a utility class for them; `className` remains for layout, which antd does not
 * contest.
 */

import Select from "@lobehub/ui/es/Select/Select";
import type { CSSProperties } from "react";

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
  /** Layout class for the trigger. Not for type — see `typography`. */
  className?: string;
  /**
   * Type scale for the trigger and for every option row, as inline style.
   * Required to be style rather than a class: see the module note on the two
   * antd rules a utility cannot outrank.
   */
  typography?: CSSProperties;
}

export function SettingsSelect({
  label,
  value,
  options,
  onValueChange,
  disabled,
  id,
  className,
  typography,
}: SettingsSelectProps) {
  return (
    <Select
      aria-label={label}
      className={className}
      styles={
        typography ? { root: typography, popup: { listItem: typography } } : undefined
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
