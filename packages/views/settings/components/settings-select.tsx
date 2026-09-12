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
 * the IANA timezone list needs — 418 zones plus the curated fallback in this
 * renderer, measured, where this comment used to say "~600". So the trade is not
 * "deprecated vs current": it is "a deprecated wrapper that can be named" against "atoms that
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
 * why a utility class cannot carry it.
 *
 * **Type is `typography`, and it is inline style, because a Tailwind utility
 * cannot win.** This is the whole reason the prop is not called `className`.
 * antd renamed the old `popupClassName` to `classNames.popup.root` and warns on
 * the old spelling
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
 * **The limit is on single-class utilities, not on our own CSS.** Tailwind emits
 * `.text-caption` as one class, which is why it stalls at 0-1-0. A composed
 * selector of ours would win both: `.orvilo-something.ant-select` is 0-2-0
 * against the trigger's 0-1-0, and 0-3-0 takes the option's 0-2-0 —
 * `packages/ui/styles/base.css` already resolves the settings row's
 * `align-items` exactly that way, and its comment is the worked example. So
 * "inline style is the only way" would be wrong, and a tab in tasks 5-9 facing
 * a *different* antd property should reach for a composed rule. What inline
 * style buys here is that the value travels with a call site's prop instead of
 * having to be restated as a selector for each usage, which is what makes
 * `typography` usable from a tab at all.
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
 *
 * **A smaller type scale also shrinks the box, so the wrapper pins the height.**
 * `.ant-select` sets no height — its box is padding + the content's
 * line-height, and the padding is a token antd derived from the *default* font
 * size, which an inline `font-size` cannot move. The timezone select came out
 * 30px tall beside two 36px siblings in the same card. `typography` therefore
 * also sets `min-height: var(--ant-select-height)` on the root, so a call site
 * changing the scale **cannot shrink the control below the control height**.
 * That is a floor, not a clamp, and the difference is worth stating: a *larger*
 * scale still raises the box, because a 16px type on a 24px line box gives
 * 6+24+6+2 = 38px beside 36px siblings. Nothing here keeps a taller control
 * level with a shorter one; only the direction that was broken is guarded.
 *
 * A call site that wants a genuinely shorter control can still say so, since
 * its own `minHeight` wins. See the `typography` prop for how that override is
 * written.
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
  /**
   * Render a filter box in the popup. For long lists only — the IANA timezone
   * timezone list is the case this exists for; a 2-to-5-item enum gets a search
   * box that is pure noise.
   *
   * This prop is why the module note above chose the antd-backed root `Select`
   * over the base-ui atoms in the first place — "search, virtua virtualisation
   * and value rendering", for exactly that list. It was chosen for a capability
   * it then never exposed, and no call site could pass it. Now it can.
   *
   * Filtering matches the label *and* the value, because the timezone options
   * are `(GMT+9) Tokyo` over `Asia/Tokyo`: someone looking for Tokyo types
   * either one. antd's own default (`optionFilterProp: "value"`) would match
   * neither of the two a user is likely to type.
   */
  search?: boolean;
  /** Layout class for the trigger. Not for type — see `typography`. */
  className?: string;
  /**
   * Type scale for the trigger and for every option row, as inline style.
   * Required to be style rather than a class: see the module note on the two
   * antd rules a utility cannot outrank.
   *
   * It also pins `min-height: var(--ant-select-height)` on the trigger, because
   * a smaller scale would otherwise shrink the whole control — `.ant-select`
   * has no height of its own. That is a floor; a larger scale still grows the
   * box.
   *
   * Both are one inline-style object, so a call site can override the floor
   * with `typography={{ minHeight: … }}` — the later key wins, no cascade
   * involved. Note the shape of that idiom: it is a layout intent expressed
   * through a prop named for type. That is a wart, and if the override path
   * ever earns its keep the prop should grow a second, honestly-named key
   * rather than staying reachable only this way.
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
  search,
  className,
  typography,
}: SettingsSelectProps) {
  return (
    <Select
      aria-label={label}
      className={className}
      filterOption={
        search
          ? (input, option) =>
              `${option?.label ?? ""} ${option?.value ?? ""}`
                .toLocaleLowerCase()
                .includes(input.toLocaleLowerCase())
          : undefined
      }
      showSearch={search}
      styles={
        typography
          ? {
              // Shrinking the type must not shrink the control. `.ant-select`
              // sets no height of its own — its box *is* padding + the
              // content's line-height — and the padding is a token antd
              // derives from the default font size, so an inline `font-size`
              // moves the line box but not the padding. A 12px timezone select
              // therefore came out 30px tall (6+16+6 + 2px border) beside two
              // 36px siblings (6+22+6 + 2), in the same card. The option rows
              // never had this problem because `.ant-select-item` carries its
              // own `min-height`.
              //
              // `min-height` rather than a line-height, which was measured and
              // rejected: the obvious token, `--ant-line-height`, is a
              // unitless ratio (1.571…) meant to multiply the *default* size,
              // so applying it to 12px yields 33px — still wrong, and now
              // wrong by a number nobody can predict. Pinning the box to the
              // control height leaves the type free.
              //
              // `--ant-select-height`, not `--ant-control-height`, and the two
              // differ exactly where it matters. Both are 36px at the default
              // size, but the select's own token is *size-aware* — antd derives
              // it from `controlHeight` for the base and switches it to
              // `controlHeightSM` / `controlHeightLG` under `&-sm` / `&-lg`
              // (`antd/es/select/style/select-input.js`), while
              // `--ant-control-height` is a flat 36px. A `min-height` beats a
              // plain `height`, so with the flat token a future small select
              // would be pinned to 36px beside siblings at 24-28px — this
              // round's misalignment returning from the other side, for a
              // control that asked to be small.
              root: { minHeight: "var(--ant-select-height)", ...typography },
              popup: { listItem: typography },
            }
          : undefined
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
