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
 * left undefined (`defaultCollapsible = isUndefined(collapsible) ? !isBorderless
 * : collapsible`, `es/Form/components/FormGroup.mjs`), so an outlined group on
 * the wide tree grows a disclosure caret: the section title becomes a
 * disclosure toggle and a stray click hides every row beneath it. That is the
 * desktop tree — when `useResponsive()` reports `mobile`, `FormGroup` returns
 * before it builds the `Collapse`, so `collapsible` is read on the desktop path
 * only and the narrow tree cannot produce an accordion. Passing
 * `collapsible={false}` here means no call site has to know that.
 *
 * An earlier revision of this note gave a second reason — that a collapsed
 * section breaks the page's nav search, "because that matches on row content".
 * It does not. The page's only search is `matches()` in `settings-page.tsx`,
 * which filters rail tabs on their value, label and description; nothing on the
 * page reads section bodies, so a collapsed section matches exactly what an
 * expanded one does.
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
import { cn } from "@orvilo/ui/lib/utils";
import { useResponsive } from "antd-style";
import { useId, type ReactNode } from "react";

export interface SettingsGroupProps {
  title?: ReactNode;
  description?: ReactNode;
  /** Trailing slot on the header row — the old `SettingsSection`'s `action`. */
  extra?: ReactNode;
  /**
   * `filled` is LobeHub's own default and the one to reach for: it draws the
   * tinted section band with an inset white card under it, border and shadow
   * included. `outlined` is the flat bordered card the old `SettingsCard` was,
   * kept for the groups that read as a plain section rather than a card;
   * `borderless` is a bare section for rows that supply their own chrome.
   *
   * The default is `filled` because that is what `lobehub/lobehub` uses — 49
   * `variant=` sites in `src/features/Settings/` against 5 `outlined` — and the
   * difference is not cosmetic. `filled` routes `FormGroup`'s `Collapse` into
   * its `filledLight` / `filledDark` compound variant, which paints
   * `colorFillQuaternary` behind the item and gives the panel `margin-inline:
   * 3px`, a border, and `staticStylish.shadow` — four stacked `box-shadow`
   * layers tinted with the border tokens. `outlined` is
   * `variantOutlinedWithoutHover` and nothing else, so it has no shadow at all.
   * That single omitted shadow is what a reader comparing the two settings
   * screens notices first.
   *
   * See `reference-lobehub-settings-parity.md` for the source lines and the
   * screenshot measurement that pins each of those values.
   */
  variant?: "filled" | "outlined" | "borderless";
  children: ReactNode;
  className?: string;
}

export function SettingsGroup({
  title,
  description,
  extra,
  variant = "filled",
  children,
  className,
}: SettingsGroupProps) {
  // A group's title is the only thing that tells two same-named rows apart —
  // "Project", "Priority" and "Due date" each appear in both create-issue
  // groups — and nothing in the DOM said so until this. `Form.Group` renders a
  // Collapse, and `@rc-component/collapse` sets the root's role only when
  // `accordion` is true — which Lobe's `FormGroup` never passes — so there was
  // no group role to name and the tests had to reach for the
  // `.ant-collapse-item` class instead.
  //
  // `role="group"` therefore has to be added by a wrapper — but not because the
  // Collapse would filter it out, which is what an earlier revision of this note
  // said. antd spreads `omit(props, ["rootClassName"])` into
  // `@rc-component/collapse` (the package whose old name, `rc-collapse`, now
  // survives only as a type-only import in Lobe's `Collapse/type.d.mts`), whose
  // root renders `pickAttrs(props, { aria: true, data: true })`, and `pickAttrs`
  // matches `role` under its aria branch (`key === "role" || match(key,
  // ariaPrefix)`, `@rc-component/util/es/pickAttrs.js`) — then spreads the result
  // *after* the root's own `role: accordion ? "tablist" : undefined`, so a role
  // passed down would survive and win on the wide tree. The wrapper is for the
  // narrow one: there `FormGroup` returns early with a `FlexBasic` that is given
  // `className` and `children` and no `rest`, so nothing a caller passes through
  // `rest` reaches the DOM below the breakpoint and the role would exist at one
  // width only.
  //
  // The name comes from `aria-labelledby` pointing at the title element rather
  // than `aria-label` repeating the string: one copy of the text cannot drift
  // from the other, and the tests then assert the same string a reader sees.
  // The wrapper carries no class and no style, so it changes no layout — the
  // settings tab body is a plain block and spacing comes from `Form.Group`'s
  // own margin.
  // An empty string is treated as no title rather than as a title that happens
  // to be empty: `role="group"` with an empty `aria-labelledby` announces a
  // group with no name, which is worse for a screen reader than no group at
  // all. A caller passing `title={someCondition ? label : ""}` gets the
  // unlabelled behaviour, not a nameless landmark.
  // `FormGroup`'s narrow tree is a different component in every respect, and it
  // drops `desc` outright: read `FormGroup.mjs` to the end and the `if (mobile)`
  // branch returns `mobileGroupHeader` + `mobileGroupBody` before `desc` is ever
  // passed to the Collapse that would render it. So on the narrow tree every
  // group lost its description — which is the copy that says what the section is
  // *for*, and on the account tab the only place two of them appear at all.
  // Measured in the renderer at a 460px viewport: the title rendered, the
  // description did not.
  //
  // **The switch is at 576px, not at antd-style's `xs` (479.98px), and the two
  // numbers are different instruments.** `createStaticStyles/responsive.js` maps
  // `xs` to `@media (max-width: 479.98px)` — that is the *CSS table*, and it
  // governs Lobe's sheets. `useResponsive()` does not read it: it goes through
  // antd's `Grid.useBreakpoint()`, whose `xs` query is
  // `(max-width: screenXSMax)` with `screenXSMax = screenSM - 1 = 575`
  // (`theme/util/alias.js`). A group between 480px and 575px is therefore on the
  // narrow *component* tree while still matching the CSS table's desktop range;
  // 576 is the boundary this branch actually flips at, and both were measured
  // either side of it.
  //
  // Descriptions are therefore rendered here, inside the group's body and ahead
  // of the rows, on the narrow tree only; the wide tree still hands `desc` to
  // `Form.Group`, which knows how to place it next to the title. It takes no
  // horizontal padding of its own: `mobileGroupBody` already carries
  // `padding-inline: 16px`, which is what lines the rows up, and an extra
  // `px-4` on top of that put the description 16px right of every label
  // beside it — measured, not assumed.
  const { mobile } = useResponsive();
  const labelId = useId();
  const labelled = title !== undefined && title !== null && title !== "";

  return (
    <div
      aria-labelledby={labelled ? labelId : undefined}
      className={cn(
        mobile && variant !== "borderless" && "orvilo-settings-group-card",
      )}
      role={labelled ? "group" : undefined}
    >
      <Form.Group
        className={className}
        collapsible={false}
        desc={mobile ? undefined : description}
        extra={extra}
        title={labelled ? <span id={labelId}>{title}</span> : undefined}
        variant={variant}
      >
        {mobile && description ? (
          <p className="text-body text-muted-foreground pt-2">{description}</p>
        ) : null}
        {children}
      </Form.Group>
    </div>
  );
}

/**
 * `Form.Item`'s row wraps by default, and Lobe only turns that off from the
 * `Form` component's own stylesheet — `styles.root` carries `.ant-row {
 * flex-wrap: nowrap }` and is applied by `Form`, which the settings page
 * deliberately never mounts (there is no page-level save, so a `Form` around
 * the tabs would claim ownership of every tab's value). Without it, a row whose
 * label and description are wide enough pushes its control onto a second line:
 * the timezone row's description plus its 288px control overflowed a 654px row
 * and stacked, while its neighbours stayed side by side.
 *
 * So the three rules that decide the row's geometry are restated for these rows
 * in `@orvilo/ui/styles/base.css`, under the class below, with the effect
 * Lobe's own `Form` would have had. Plain CSS rather than utilities: each rule
 * has to land on an antd *child* of the item, which a class on the item itself
 * cannot reach, and this way none of them depends on Tailwind emitting a
 * candidate. The fourth difference is not CSS at all — antd turns the label
 * colon on unless it is told otherwise, so `Form.Item` gets `colon={false}`.
 */
const ROW_CLASS = "orvilo-settings-row";

/** See `ROW_CLASS`: the other half of the row's alignment, also in `base.css`. */
const ROW_ALIGN_START_CLASS = "orvilo-settings-row-start";

/** See `disabled` on the props: dims the label, also in `base.css`. */
const ROW_DISABLED_CLASS = "orvilo-settings-row-disabled";

export interface SettingsFormRowProps {
  label?: ReactNode;
  description?: ReactNode;
  /**
   * Id of the control this row labels — the old `SettingsRow`'s `htmlFor`.
   *
   * antd always renders the row's label as a real `<label>`, but it mints the
   * `for` from the item's **field id**, which only exists for an item that has
   * a `name` (`antd/es/form/FormItem/ItemHolder.js`: `htmlFor: fieldId`). The
   * state-ownership rule forbids a `name` here — antd would then own the value
   * nothing writes to — so without this the label is a `<label>` that points at
   * nothing, and a bare `Input` in the control slot has no accessible name at
   * all. The two controls that *can* name themselves (`SettingsSwitch`,
   * `SettingsSelect`) carry their own `aria-label`; this is for the text fields
   * that cannot.
   *
   * It is association only, not value ownership: passing `htmlFor` alongside an
   * `id` on the control gives antd nothing to store.
   */
  htmlFor?: string;
  /**
   * The control column's width, in pixels. The old `SettingsRow` used five
   * container-query tiers; these are the same tiers restated as the pixels they
   * resolved to.
   *
   * **The width is exact, but not because of this prop.** `Form.Item` turns it
   * into `.ant-form-item-control { width: var(--form-item-min-width) !important
   * }`, and that declaration is only a *base size*: a flex item's `width` is
   * where it starts, not what it gets, and Lobe leaves the column at
   * `flex: 0 1 auto` (`itemStyles.root` sets `flex: unset` on the row's
   * children), so on its own the column shrinks — measured, the account tab's
   * long labels resolved their neighbours to 384/327/271/375px in one card.
   * What makes it exact is `packages/ui/styles/base.css`, which pins
   * `flex: 0 0 auto` on the control and `flex: 1 1 0%` on the label above the
   * stacking breakpoint. Lobe's own pair of rules exists but only under
   * antd-style's `responsive.sm`, which is `@media (max-width: 575.98px)`.
   */
  minWidth?: number;
  /**
   * Marks the row as disabled so its label dims, for a row whose control is
   * `disabled` because the viewer may not edit it — a non-managing workspace
   * member, say.
   *
   * The old rows got this from a `data-disabled` attribute on a base-ui
   * `Field`, whose `FieldLabel` carries
   * `group-data-[disabled=true]/field:opacity-50`. These rows have no `Field`
   * and no `FieldLabel`, so re-adding that attribute would have been dead — the
   * variant needs a `group/field` ancestor and a `[data-slot=field-label]`
   * target, and an antd row has neither. The class below is the replacement,
   * and its rule lives with the rest of this row's geometry in `base.css`.
   */
  disabled?: boolean;
  /**
   * Where the control sits when the row is taller than the label. `center` is
   * the default Lobe's own `Form` gives its rows; `start` is for a row whose
   * control is a block of its own, such as a shortcut recorder with an error
   * line under it.
   */
  align?: "start" | "center";
  /** Draw a separator above this row. */
  divider?: boolean;
  children: ReactNode;
  className?: string;
}

export function SettingsFormRow({
  label,
  description,
  htmlFor,
  minWidth,
  align = "center",
  divider,
  disabled,
  children,
  className,
}: SettingsFormRowProps) {
  return (
    <Form.Item
      className={cn(
        ROW_CLASS,
        align === "start" && ROW_ALIGN_START_CLASS,
        disabled && ROW_DISABLED_CLASS,
        className,
      )}
      colon={false}
      desc={description}
      divider={divider}
      htmlFor={htmlFor}
      label={label}
      minWidth={minWidth}
    >
      {children}
    </Form.Item>
  );
}
