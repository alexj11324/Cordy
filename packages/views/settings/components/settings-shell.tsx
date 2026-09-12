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
import { cn } from "@orvilo/ui/lib/utils";
import { useId, type ReactNode } from "react";

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
  // A group's title is the only thing that tells two same-named rows apart —
  // "Project", "Priority" and "Due date" each appear in both create-issue
  // groups — and nothing in the DOM said so until this. `Form.Group` renders a
  // Collapse, and rc-collapse sets a role only when `accordion` is true, so
  // there was no group role to name and the tests had to reach for the
  // `.ant-collapse-item` class instead.
  //
  // `role="group"` therefore has to be added by a wrapper. It cannot be put on
  // the Collapse root: antd spreads `omit(props, ["rootClassName"])` into
  // rc-collapse, which renders `pickAttrs(props, { aria: true, data: true })` —
  // a whitelist that lets `aria-*` through and drops `role` entirely.
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
  const labelId = useId();
  const labelled = title !== undefined && title !== null && title !== "";

  return (
    <div
      aria-labelledby={labelled ? labelId : undefined}
      role={labelled ? "group" : undefined}
    >
      <Form.Group
        className={className}
        collapsible={false}
        desc={description}
        extra={extra}
        title={labelled ? <span id={labelId}>{title}</span> : undefined}
        variant={variant}
      >
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
   * The control column's width, in pixels — an exact width, not a floor:
   * `Form.Item` turns it into
   * `.ant-form-item-control { width: var(--form-item-min-width) !important }`.
   * The old `SettingsRow` used five container-query tiers; these are the same
   * tiers restated as the pixels they resolved to.
   */
  minWidth?: number;
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
  children,
  className,
}: SettingsFormRowProps) {
  return (
    <Form.Item
      className={cn(ROW_CLASS, align === "start" && ROW_ALIGN_START_CLASS, className)}
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
