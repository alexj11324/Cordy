"use client";

/**
 * The settings page's switch, built on Lobe's base-ui Switch **atoms** rather
 * than on the `Switch` component that wraps them.
 *
 * `Switch` takes a fixed prop list and spreads nothing else onto the DOM
 * (`es/base-ui/Switch/Switch.mjs` destructures every prop it accepts and passes
 * only those to `SwitchRoot`), so `aria-label` handed to it is dropped on the
 * floor — and `SwitchProps` does not extend the button attributes, so
 * TypeScript rejects the attempt outright. The atom underneath,
 * `SwitchRoot`, does `render: <button {...rest} …>`, so every remaining prop
 * reaches the `<button role="switch">`.
 *
 * That matters here because nothing else can name these controls. antd's
 * `Form.Item` renders its `label` as a `<label>` with no `for`: it only mints
 * one for a field that has a `name`, and the state-ownership rule in
 * `reference-lobe-tab-migration.md` is explicit that a control whose value *is*
 * the persisted value must not have one. So a switch inside a `SettingsFormRow`
 * would otherwise be announced as an unnamed "switch" — hence `label` is
 * required, not optional.
 *
 * Imported from the deep path for the same fan-out reason as the shell's
 * `Form`: every migrated tab renders rows of switches, and a module's graph is
 * paid once per test file that imports it.
 */

import { SwitchRoot, SwitchThumb } from "@lobehub/ui/es/base-ui/Switch/atoms";

export interface SettingsSwitchProps {
  /** Accessible name for the switch. Required — see the module note. */
  label: string;
  checked: boolean;
  onCheckedChange: (checked: boolean) => void;
  disabled?: boolean;
  id?: string;
  className?: string;
}

export function SettingsSwitch({
  label,
  checked,
  onCheckedChange,
  disabled,
  id,
  className,
}: SettingsSwitchProps) {
  return (
    <SwitchRoot
      aria-label={label}
      checked={checked}
      className={className}
      disabled={disabled}
      id={id}
      onCheckedChange={onCheckedChange}
    >
      <SwitchThumb />
    </SwitchRoot>
  );
}
