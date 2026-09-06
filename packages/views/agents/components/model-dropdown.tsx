"use client";

import { useEffect, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { ChevronDown, Cpu } from "lucide-react";
import { runtimeModelsOptions } from "@patchbay/core/runtimes";
import {
  Popover,
  PopoverTrigger,
  PopoverContent,
} from "@patchbay/ui/components/ui/popover";
import { Label } from "@patchbay/ui/components/ui/label";
import { cn } from "@patchbay/ui/lib/utils";
import { useT } from "../../i18n";
import {
  ModelSelectorContent,
  type ModelSelection,
  type ModelSelectorRuntime,
} from "./model-selector-content";

export interface ModelDropdownProps {
  runtimeId: string | null;
  runtimeOnline: boolean;
  value: string;
  onChange: (value: string) => Promise<void> | void;
  disabled?: boolean;
  provider?: string;
  thinkingLevel?: string;
  runtimes?: ModelSelectorRuntime[];
  onSelection?: (selection: ModelSelection) => Promise<void> | void;
  showLabel?: boolean;
  variant?: "field" | "chip";
  clearUnsupported?: boolean;
  inline?: boolean;
  allowEffort?: boolean;
}

export function ModelDropdown({
  runtimeId,
  runtimeOnline,
  value,
  onChange,
  disabled,
  provider = "",
  thinkingLevel = "",
  runtimes,
  onSelection,
  showLabel = true,
  variant = "field",
  clearUnsupported = true,
  inline = false,
  allowEffort,
}: ModelDropdownProps) {
  const { t } = useT("agents");
  const [open, setOpen] = useState(false);
  const modelsQuery = useQuery(
    runtimeModelsOptions(runtimeOnline ? runtimeId : null, runtimes?.find((runtime) => runtime.id === runtimeId)?.workspace_id),
  );
  const supported = modelsQuery.data?.supported ?? true;
  const effortEditable = allowEffort ?? Boolean(onSelection);
  useEffect(() => {
    if (clearUnsupported && !supported && value) void onChange("");
  }, [clearUnsupported, supported, value, onChange]);
  const choices = runtimes ?? [
    {
      id: runtimeId ?? "",
      name: provider || t(($) => $.model_dropdown.label),
      provider,
      status: runtimeOnline ? ("online" as const) : ("offline" as const),
    },
  ];
  const selectedModel = modelsQuery.data?.models.find(
    (item) => item.id === value,
  );
  const effortLabel =
    selectedModel?.thinking?.supported_levels.find(
      (level) => level.value === thinkingLevel,
    )?.label ?? thinkingLevel;
  const modelLabel = value
    ? (selectedModel?.label ?? value)
    : t(($) =>
        disabled
          ? $.model_dropdown.select_runtime_first
          : runtimeOnline
            ? $.model_dropdown.default_provider
            : $.model_dropdown.runtime_offline_manual,
      );
  const triggerLabel = `${modelLabel}${effortLabel ? ` (${effortLabel})` : ""}`;

  if (inline) {
    return (
      <fieldset
        disabled={disabled}
        className="min-w-0 overflow-hidden rounded-lg border border-border bg-background p-0 disabled:opacity-60"
      >
        <ModelSelectorContent
          key={runtimeId}
          runtimes={choices}
          runtimeId={runtimeId ?? ""}
          model={value}
          thinkingLevel={thinkingLevel}
          allowEffort={effortEditable}
          className="h-96"
          autoFocus={false}
          preferFavorites={false}
          onSelect={async (selection) => {
            if (onSelection) await onSelection(selection);
            else await onChange(selection.model);
          }}
        />
      </fieldset>
    );
  }

  return (
    <div className="flex min-w-0 flex-col">
      {showLabel && (
        <Label className="text-caption text-muted-foreground">
          {t(($) => $.model_dropdown.label)}
        </Label>
      )}
      {!supported && !modelsQuery.isLoading && choices.length <= 1 ? (
        <div className="mt-1.5 rounded-lg border border-dashed px-3 py-2.5 text-caption text-muted-foreground">
          {t(($) => $.model_dropdown.managed_by_runtime_title)}
        </div>
      ) : (
        <Popover open={open} onOpenChange={setOpen}>
          <PopoverTrigger
            disabled={disabled}
            aria-label={t(($) => $.pickers.model_tooltip, {
              value: triggerLabel,
            })}
            className={cn(
              "flex min-w-0 items-center gap-2 text-left transition-colors hover:bg-muted focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-50",
              showLabel && "mt-1.5",
              variant === "field"
                ? "min-h-10 w-full rounded-lg border border-input bg-transparent px-3 py-2.5 text-body"
                : "rounded px-1.5 py-0.5 text-micro",
            )}
          >
            <Cpu
              className="size-4 shrink-0 text-muted-foreground"
              aria-hidden
            />
            <span className="min-w-0 flex-1 truncate">{triggerLabel}</span>
            <ChevronDown
              className="size-3.5 shrink-0 text-muted-foreground"
              aria-hidden
            />
          </PopoverTrigger>
          <PopoverContent
            align="start"
            className="w-[min(35rem,calc(100vw-1rem))] gap-0 overflow-hidden p-0"
          >
            {open && (
              <ModelSelectorContent
                key={runtimeId}
                runtimes={choices}
                runtimeId={runtimeId ?? ""}
                model={value}
                thinkingLevel={thinkingLevel}
                allowEffort={effortEditable}
                onSelect={async (selection) => {
                  if (onSelection) await onSelection(selection);
                  else await onChange(selection.model);
                  setOpen(false);
                }}
              />
            )}
          </PopoverContent>
        </Popover>
      )}
    </div>
  );
}
