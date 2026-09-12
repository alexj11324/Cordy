"use client";

/**
 * The save-state line a settings field shows while it persists on its own.
 *
 * Lifted verbatim out of `settings-layout`: the three tabs that render it are
 * live, so the hand-rolled atom layer could not be deleted while the component
 * lived inside it. Nothing here is Lobe-specific — it is a plain
 * `<span role="status">` with lucide icons — so it survives the migration
 * unchanged.
 *
 * It sits next to `settings-save-status`, which owns the `SettingsSaveStatus`
 * contract this component renders and which already outlived the old home.
 */

import { AlertCircle, Check, Loader2 } from "lucide-react";
import { cn } from "@orvilo/ui/lib/utils";
import type { SettingsSaveStatus } from "./settings-save-status";

export function SettingsSaveState({
  status,
  savingLabel,
  savedLabel,
  errorLabel,
}: {
  status: SettingsSaveStatus;
  savingLabel: string;
  savedLabel: string;
  errorLabel: string;
}) {
  if (status === "idle") return null;

  const content =
    status === "saving" ? (
      <>
        <Loader2 className="size-3 animate-spin" />
        {savingLabel}
      </>
    ) : status === "saved" ? (
      <>
        <Check className="size-3 text-success" />
        {savedLabel}
      </>
    ) : (
      <>
        <AlertCircle className="size-3 text-destructive" />
        {errorLabel}
      </>
    );

  return (
    <span
      role="status"
      className={cn(
        "inline-flex items-center gap-1.5 text-caption text-muted-foreground",
        status === "error" && "text-destructive",
      )}
    >
      {content}
    </span>
  );
}
