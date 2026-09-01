"use client";

import { useState } from "react";
import { ChevronDown } from "lucide-react";
import type { RuntimeSessionMode } from "@patchbay/core/types";
import { pickerSessionModes } from "@patchbay/core/runtimes";
import {
  PickerItem,
  PropertyPicker,
} from "../../issues/components/pickers";
import { useT } from "../../i18n";

const TRIGGER_CLASS =
  "inline-flex h-7 max-w-[12rem] min-w-0 cursor-pointer items-center gap-1 rounded-md px-1.5 text-caption text-muted-foreground transition-colors hover:bg-accent hover:text-foreground";

/**
 * Protocol-discovery session mode picker. Always offers full access (empty
 * value = daemon yolo). Additional rows come only from advertised
 * `auto_review` / `value === "auto"` catalog entries — never from a
 * provider-name branch.
 */
export function SessionModePicker({
  value,
  advertised,
  canEdit = true,
  onChange,
}: {
  value: string;
  advertised: readonly RuntimeSessionMode[] | undefined;
  canEdit?: boolean;
  onChange: (next: string) => Promise<void> | void;
}) {
  const { t } = useT("chat");
  const [open, setOpen] = useState(false);
  const modes = pickerSessionModes(advertised);
  const selected = value ? modes.find((mode) => mode.value === value) : undefined;
  const triggerLabel = selected
    ? selected.label
    : value || t(($) => $.control_bar.session_mode_full_access);
  const triggerTitle = t(($) => $.control_bar.session_mode_tooltip, {
    value: triggerLabel,
  });

  const select = async (next: string) => {
    setOpen(false);
    if (next !== value) await onChange(next);
  };

  if (!canEdit) {
    return (
      <span className="inline-flex h-7 max-w-[12rem] min-w-0 items-center truncate px-1.5 text-caption text-muted-foreground" title={triggerTitle}>
        {triggerLabel}
      </span>
    );
  }

  return (
    <PropertyPicker
      open={open}
      onOpenChange={setOpen}
      width="w-auto min-w-[14rem] max-w-md"
      align="end"
      tooltip={triggerTitle}
      triggerRender={
        <button type="button" className={TRIGGER_CLASS} aria-label={triggerTitle} />
      }
      trigger={
        <>
          <span className="min-w-0 truncate">{triggerLabel}</span>
          <ChevronDown className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
        </>
      }
    >
      <PickerItem selected={!value} onClick={() => void select("")}>
        <span className="block min-w-0 flex-1 text-left">
          <span className="truncate text-label font-medium">
            {t(($) => $.control_bar.session_mode_full_access)}
          </span>
        </span>
      </PickerItem>
      {modes.map((mode) => (
        <PickerItem
          key={mode.value}
          selected={mode.value === value}
          onClick={() => void select(mode.value)}
        >
          <span className="block min-w-0 flex-1 text-left">
            <span className="truncate text-label font-medium">{mode.label}</span>
          </span>
        </PickerItem>
      ))}
    </PropertyPicker>
  );
}
