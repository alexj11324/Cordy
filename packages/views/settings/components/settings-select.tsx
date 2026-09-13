"use client";

/**
 * The settings page's single-choice select.
 *
 * **Why the antd-backed root `Select` and not `@lobehub/ui/base-ui`'s.** The
 * base-ui component is the one the library points at, but it takes no aria
 * props: its parameter list is fixed and it spreads nothing
 * (`es/base-ui/Select/Select.mjs` — `memo(({ allowClear, autoFocus, …, virtual }) => …)`),
 * so an `aria-label` never reaches the `<button role="combobox">`. Measured: a
 * visually-hidden label in the `prefix` slot does reach the DOM and still leaves
 * the trigger with an empty accessible name.
 *
 * **This paragraph made two further claims, and both were false — checked
 * against the installed packages, not reasoned about.** It said the base-ui
 * select "gives its trigger no way to be named", and that a base-ui `Field.Root`
 * "is not available here at all, since Lobe's base-ui barrel exports neither
 * `Field` nor `SelectLabel`". In fact the trigger *is* reachable for a name:
 * Lobe forwards `id` to base-ui's `Select.Root`, base-ui stores it through
 * `useLabelableId` (`@base-ui/react/select/root/SelectRoot.js`) and
 * `SelectTrigger` puts `idProp ?? rootId` on the trigger
 * (`select/trigger/SelectTrigger.js`), so an associated `<label for>` names it.
 * And `Field` **is** exported: `@lobehub/ui/base-ui/Form` exports
 * `Field: FormField` (`es/base-ui/Form/index.mjs`), which is itself built on
 * `@base-ui/react/field`'s `Field.Root`/`Field.Label`
 * (`es/base-ui/Form/components/FormField.mjs`), with the same `useLabelableId`
 * making a `Field`-wrapped select labelled. So the gap is prop-level
 * (`aria-label`) and not a dead end, and **the reason to prefer the
 * antd-backed `Select` is the costs below — not the name.** A future tab that
 * wants the base-ui select and can afford a wrapping `Field` is not blocked by
 * anything here.
 *
 * **A third path does exist and is deliberately not taken.** The trigger *atom*
 * (`SelectTrigger`) does forward `aria-label` and does produce a real
 * accessible name. What it costs is everything the `Select` component supplies
 * on top of the atoms — search, windowed rendering of a long list, value
 * rendering — which the IANA timezone list needs. That is a source claim, not a
 * guess: `es/base-ui/Select/atoms.mjs` exports Root, Backdrop, Separator,
 * Trigger, Icon, Value, Portal, Positioner, Popup, List, Item, ItemText,
 * ItemIndicator, Group, GroupLabel, the two scroll arrows and Arrow, and nothing
 * else — the search input and the virtualisation live in `parts.mjs` and
 * `hooks.mjs`, neither of which is exported. So the trade is not "deprecated vs
 * current": it is "a wrapper that cannot take an aria prop but can be labelled"
 * against "atoms that can be named but that we would have to re-implement". If a
 * future surface needs a nameable select over a *short* list, the atoms are the
 * right answer there.
 *
 * **How the long list is rendered: `@rc-component/virtual-list`.** rc-select
 * imports that package directly (`@rc-component/select/es/OptionList.js`) and
 * hands it a `prefixCls` of its own, which is how the DOM ends up with
 * `.ant-select-dropdown-list-holder`: that class is the virtual list's scrolling
 * holder, `${prefixCls}-holder` in `@rc-component/virtual-list/es/List.js`.
 * Three numbers, kept apart because they are three different things:
 * `Intl.supportedValuesOf("timeZone")` returns **418** in this renderer's
 * Chromium (this comment once said "~600"); `timezoneOptions()` unions that set
 * with the curated fallback, the current zone and the browser's, so the
 * preferences select holds **421** rows; and **9** of those are in the DOM at any
 * moment, because a virtual list renders a window. A node count is the window;
 * `scrollHeight / rowHeight` is the list. That distinction is load-bearing for
 * the smoke check that guards `search`, which measures the holder — a partial
 * filter leaves the window at 9 and would have been read as "did not narrow".
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
 * `label` is a plain string for the same reason: rc-select copies it into the
 * option's `title` attribute (`es/OptionList.js` — `title: optionTitle`, taken
 * from the option's content when that content is a string or number, which a
 * plain-string `label` always is), and that attribute is the only handle a test
 * can take on an option. (Measured in the renderer: three `.ant-select-content`
 * nodes for the three selects on the preferences tab, and **zero**
 * `.ant-select-selector` and **zero** `.ant-select-selection-item` anywhere in
 * the document — v6 carries the value in `-content` and renders no `-selector`
 * at all. `-selection-item` still has rules in `antd/es/select/style/`, for
 * other shapes than ours, which is why a probe should accept both spellings
 * rather than trusting the stylesheet.) The popup's items are plain `div`s — antd v6 exposes
 * the options to assistive technology through a separate, clipped `[role=option]`
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
 * `...contextStyles?.popup?.listItem` on the same element antd styles
 * (`@rc-component/select/es/OptionList.js`), and the trigger's is
 * `style: mergedStyles.root` (`antd/es/select/index.js`) — and inline style
 * outranks both competing rules regardless of specificity or sheet order, with
 * no `!important` to maintain. The trigger's selected value follows for free: its
 * `.ant-select-content` has no `font-size` rule of its own (checked across
 * `antd/es/select/style/`), so it inherits.
 *
 * The values are still tokens (`var(--text-caption)`, `var(--font-mono)`), so a
 * call site names the design scale, not a pixel count. What it cannot do is use
 * a utility class for them; `className` remains for layout, which antd does not
 * contest.
 *
 * **A smaller type scale also shrinks the box, so the wrapper pins the height.**
 * `.ant-select` sets no height — its box is padding + the content's line-height.
 * The padding is the part an inline `font-size` cannot move, and the source says
 * why: `antd/es/select/style/select-input.js` declares
 * `--ant-select-padding-vertical: calc((--ant-select-height - --ant-select-font-height) / 2 - border)`,
 * and `font-height` is the `fontHeight` **token**, not the rendered text — so it
 * stays at the default size while the line box shrinks. The timezone select came out
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
   * Make the trigger typeable so the option list narrows as you type. For long
   * lists only — the IANA timezone list is the case this exists for; a
   * 2-to-5-item enum gets a filter that is pure noise.
   *
   * There is **no input inside the popup**: the caret lives in the trigger
   * (`SelectInput/Input.js`), and that input is the element carrying
   * `role="combobox"` — measured, the combobox in the renderer is the trigger's
   * `.ant-select-input`. A reader looking for a second control in the dropdown
   * will not find one.
   *
   * Search is one of the three capabilities the module note above names as the
   * reason for the antd-backed root `Select` over the base-ui atoms — "search,
   * windowed rendering of a long list, value rendering" — for exactly that list.
   * It was chosen for a capability it then never exposed, and no call site could
   * pass it. Now it can.
   *
   * Filtering matches the label *and* the value, because the timezone options
   * are `(GMT+9) Tokyo` over `Asia/Tokyo`: someone looking for Tokyo types
   * either one.
   *
   * The custom `filterOption` is needed for the label half, and the source says
   * exactly how much: rc-select's fallback filter
   * (`@rc-component/select/es/hooks/useFilterOptions.js`) tests
   * `optionFilterProp` when given, and otherwise auto-selects — but an option
   * *object* like ours has no `children`, so it falls through to
   * `includes(option.value, search)`. Label-only text therefore matches nothing
   * by default: `(GMT+9)` on this tab, and `（浏览器）` on the preferences tab's
   * browser entry. Both are visible in the row the user is reading, which is the
   * whole reason to search by label.
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
              // unitless ratio — `1.5714285714285714`, read off the select in
              // the renderer, where the bridge declares it (it is empty at
              // `:root`; the tokens are scoped to the bridge's subtree) — meant
              // to multiply the *default* size, so applying it to 12px yields
              // 33px: still wrong, and now wrong by a number nobody can
              // predict. Pinning the box to the control height leaves the type
              // free.
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
