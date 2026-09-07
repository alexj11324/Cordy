"use client";

import { useT } from "../../../i18n";
import { ModelDropdown, type ModelDropdownProps } from "../model-dropdown";

/** General settings and create flows share the same provider/model/effort picker. */
export function ModelPicker({
  canEdit = true,
  variant = "chip",
  ...props
}: Omit<ModelDropdownProps, "disabled"> & { canEdit?: boolean }) {
  const { t } = useT("agents");
  if (!canEdit)
    return (
      <span className="min-w-0 truncate text-body text-muted-foreground">
        {props.value || "—"}
        {props.thinkingLevel ? ` (${props.thinkingLevel})` : ""}
        {props.serviceTier ? ` · ${t(($) => $.inspector.prop_speed)}: ${props.serviceTier}` : ""}
      </span>
    );
  return (
    <ModelDropdown
      {...props}
      variant={variant}
      disabled={!canEdit}
      clearUnsupported={false}
    />
  );
}
