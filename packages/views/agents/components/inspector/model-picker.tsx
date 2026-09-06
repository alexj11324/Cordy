"use client";

import { ModelDropdown, type ModelDropdownProps } from "../model-dropdown";

/** General settings and create flows share the same provider/model/effort picker. */
export function ModelPicker({
  canEdit = true,
  variant = "chip",
  ...props
}: Omit<ModelDropdownProps, "disabled"> & { canEdit?: boolean }) {
  if (!canEdit)
    return (
      <span className="min-w-0 truncate text-body text-muted-foreground">
        {props.value || "—"}
        {props.thinkingLevel ? ` (${props.thinkingLevel})` : ""}
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
