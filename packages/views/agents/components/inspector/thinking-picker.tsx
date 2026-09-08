"use client";

import { useState } from "react";
import { Brain, ChevronDown } from "lucide-react";
import type { RuntimeModelThinkingLevel } from "@orvilo/core/types";
import { Label } from "@orvilo/ui/components/ui/label";
import {
  PickerItem,
  PropertyPicker,
} from "../../../issues/components/pickers";
import { CHIP_CLASS } from "./chip";
import { useT } from "../../../i18n";

/**
 * Per-agent reasoning/effort picker (MUL-2339). Renders only when the
 * current model exposes a non-empty `supported_levels` set — Claude, Codex,
 * and OpenCode today; every other provider gets nothing. The catalog is daemon-
 * discovered, so the value/label pairs match each CLI's own UI (`Low`,
 * `Extra high`, …) verbatim; never normalised across providers.
 *
 * Empty string means no thinking effort has been chosen yet. We do not
 * treat that as "follow the local CLI" — the four-column selector persists
 * a real catalog level when one exists.
 */
export function ThinkingPicker({
  value,
  levels,
  canEdit = true,
  variant = "chip",
  showLabel = true,
  onChange,
}: {
  /** Persisted thinking_level — "" means none chosen yet. */
  value: string;
  /** Supported levels for the current (runtime, model) pair. Usually
   *  non-empty when the row is shown, but the stale-orphan clear path
   *  in ThinkingPropRow mounts the picker with an empty list plus a
   *  persisted value so the user can see and clear the dangling token. */
  levels: RuntimeModelThinkingLevel[];
  /** When false, render a static read-only display and skip the popover. */
  canEdit?: boolean;
  variant?: "chip" | "field";
  showLabel?: boolean;
  onChange: (next: string) => Promise<void> | void;
}) {
  const { t } = useT("agents");
  const [open, setOpen] = useState(false);

  const selected = value ? levels.find((l) => l.value === value) : undefined;
  // Unknown-but-set value (model swap that dropped the option, CLI upgrade
  // that trimmed the catalog): show the raw token so the user can see what
  // is actually persisted and clear it, rather than silently labelling it
  // "Default" when the backend would still send the stale value.
  const triggerLabel = selected
    ? selected.label
    : value || t(($) => $.pickers.thinking_choose);
  const triggerTitle = t(($) => $.pickers.thinking_tooltip, {
    value: triggerLabel,
  });

  const select = async (next: string) => {
    setOpen(false);
    if (next !== value) await onChange(next);
  };

  if (!canEdit) {
    if (variant === "field") {
      const control = (
        <div className="flex min-h-10 items-center gap-2 rounded-lg border border-input bg-input/50 px-3 text-body text-muted-foreground">
          <Brain className="h-4 w-4 shrink-0" aria-hidden="true" />
          <span className="min-w-0 truncate">{triggerLabel}</span>
        </div>
      );
      if (!showLabel) return control;
      return (
        <div className="flex min-w-0 flex-col">
          <Label>{t(($) => $.inspector.prop_thinking)}</Label>
          <div className="mt-1.5">{control}</div>
        </div>
      );
    }
    return (
      <span
        className="min-w-0 truncate px-1.5 py-0.5 font-mono text-micro text-muted-foreground"
        title={triggerTitle}
      >
        {triggerLabel}
      </span>
    );
  }

  const picker = (
    <PropertyPicker
      open={open}
      onOpenChange={setOpen}
      width={
        variant === "field"
          ? "w-[var(--anchor-width)] min-w-[14rem] max-w-md"
          : "w-auto min-w-[14rem] max-w-md"
      }
      align="start"
      tooltip={triggerTitle}
      triggerRender={
        <button
          type="button"
          className={
            variant === "field"
              ? `${showLabel ? "mt-1.5 " : ""}flex min-h-10 w-full min-w-0 items-center gap-2 rounded-lg border border-input bg-transparent px-3 text-left text-body transition-colors hover:bg-muted focus-visible:outline-none focus-visible:ring-3 focus-visible:ring-ring/50`
              : CHIP_CLASS
          }
          aria-label={triggerTitle}
        />
      }
      trigger={
        <>
          {variant === "field" ? (
            <Brain
              className="h-4 w-4 shrink-0 text-muted-foreground"
              aria-hidden="true"
            />
          ) : null}
          <span
            className={
              variant === "field"
                ? "min-w-0 flex-1 truncate"
                : "min-w-0 truncate font-mono text-micro"
            }
          >
            {triggerLabel}
          </span>
          {variant === "field" ? (
            <ChevronDown
              className={`h-4 w-4 shrink-0 text-muted-foreground transition-transform ${
                open ? "rotate-180" : ""
              }`}
              aria-hidden="true"
            />
          ) : null}
        </>
      }
    >
      {levels.map((l) => (
        <PickerItem
          key={l.value}
          selected={l.value === value}
          onClick={() => void select(l.value)}
        >
          {/* PickerItem wraps children in a flex `<span>`. Putting a
              `<div>` inside that <span> is block-in-inline (invalid HTML5)
              and triggers browser quirks that shift descendant x-position.
              Use a `<span>` with explicit `block` + `text-left` so layout
              is deterministic across rows. */}
          <span className="block min-w-0 flex-1 text-left">
            <span className="truncate text-label font-medium">{l.label}</span>
            {l.description && (
              <span className="mt-0.5 block text-micro leading-snug text-muted-foreground">
                {l.description}
              </span>
            )}
          </span>
        </PickerItem>
      ))}

      {value && (
        <button
          type="button"
          onClick={() => void select("")}
          className="mt-1 flex w-full items-center border-t px-3 py-2 text-left text-caption text-muted-foreground transition-colors hover:bg-accent/50"
          title={t(($) => $.pickers.thinking_clear_title)}
        >
          {t(($) => $.pickers.thinking_clear)}
        </button>
      )}
    </PropertyPicker>
  );

  if (variant === "field") {
    if (!showLabel) return picker;
    return (
      <div className="flex min-w-0 flex-col">
        <Label>{t(($) => $.inspector.prop_thinking)}</Label>
        {picker}
      </div>
    );
  }

  return picker;
}
