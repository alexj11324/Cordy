"use client";

import { useEffect, useState, type ReactNode } from "react";
import { useQuery } from "@tanstack/react-query";
import { ChevronDown, Cpu } from "lucide-react";
import { runtimeModelsOptions } from "@orvilo/core/runtimes";
import {
  ModelSelector as AIModelSelector,
  ModelSelectorContent as AIModelSelectorContent,
  ModelSelectorTrigger as AIModelSelectorTrigger,
} from "@orvilo/ui/components/ai-elements/model-selector";
import { Label } from "@orvilo/ui/components/ui/label";
import { cn } from "@orvilo/ui/lib/utils";
import { ProviderLogo } from "../../runtimes/components/provider-logo";
import { useT } from "../../i18n";
import {
  ModelSelectorContent,
  type ModelSelection,
  type ModelSelectorRuntime,
} from "./model-selector-content";
import { serviceTierDisplayName } from "./model-selector-options";

export interface ModelDropdownProps {
  runtimeId: string | null;
  runtimeOnline: boolean;
  value: string;
  onChange: (value: string) => Promise<void> | void;
  disabled?: boolean;
  provider?: string;
  thinkingLevel?: string;
  serviceTier?: string;
  runtimes?: ModelSelectorRuntime[];
  onSelection?: (selection: ModelSelection) => Promise<void> | void;
  showLabel?: boolean;
  variant?: "field" | "chip";
  clearUnsupported?: boolean;
  allowEffort?: boolean;
  allowSpeed?: boolean;
  popoverAlign?: "start" | "center" | "end";
}

export function ModelDropdown({
  runtimeId,
  runtimeOnline,
  value,
  onChange,
  disabled,
  provider = "",
  thinkingLevel = "",
  serviceTier = "",
  runtimes,
  onSelection,
  showLabel = true,
  variant = "field",
  clearUnsupported = true,
  allowEffort,
  allowSpeed,
}: ModelDropdownProps) {
  const { t } = useT("agents");
  const [open, setOpen] = useState(false);
  const modelsQuery = useQuery(
    runtimeModelsOptions(
      runtimeOnline ? runtimeId : null,
      runtimes?.find((runtime) => runtime.id === runtimeId)?.workspace_id,
    ),
  );
  const supported = modelsQuery.data?.supported ?? true;
  const effortEditable = allowEffort ?? Boolean(onSelection);
  const speedEditable = allowSpeed ?? effortEditable;
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
  const selectedRuntime = choices.find((runtime) => runtime.id === runtimeId);
  const logoProvider = selectedRuntime?.provider || provider;
  const selectedModel = modelsQuery.data?.models.find(
    (item) => item.id === value,
  );
  const effortLabel = effortEditable
    ? (selectedModel?.thinking?.supported_levels.find(
        (level) => level.value === thinkingLevel,
      )?.label ?? (thinkingLevel || ""))
    : "";
  const speedLabel = speedEditable
    ? serviceTierDisplayName(
        serviceTier,
        selectedModel,
        t(($) => $.pickers.service_tier_standard),
      )
    : "";
  const modelLabel = value
    ? (selectedModel?.label ?? value)
    : t(($) =>
        disabled
          ? $.model_dropdown.select_runtime_first
          : runtimeOnline
            ? $.model_dropdown.default_provider
            : $.model_dropdown.runtime_offline_manual,
      );
  const triggerLabel = [modelLabel, effortLabel, speedLabel]
    .filter(Boolean)
    .join(" · ");

  const trigger = (
    <span className="inline-flex min-w-0 items-center gap-1.5">
      {logoProvider ? (
        <ProviderLogo provider={logoProvider} className="size-4 shrink-0" />
      ) : (
        <Cpu className="size-4 shrink-0 text-muted-foreground" aria-hidden />
      )}
      <span className="min-w-0 truncate">{modelLabel}</span>
      {effortLabel ? <TriggerMeta>{effortLabel}</TriggerMeta> : null}
      {speedLabel ? <TriggerMeta>{speedLabel}</TriggerMeta> : null}
      <ChevronDown
        className={cn(
          "size-3.5 shrink-0 text-muted-foreground transition-transform duration-200",
          open && "rotate-180",
        )}
        aria-hidden
      />
    </span>
  );

  return (
    <div className="flex min-w-0 flex-col">
      {showLabel && (
        <Label className="text-caption text-muted-foreground">
          {t(($) => $.model_dropdown.label)}
        </Label>
      )}
      {!supported && !modelsQuery.isLoading && choices.length <= 1 ? (
        <div
          className={cn(
            triggerClassName(variant, showLabel),
            "cursor-default text-muted-foreground hover:bg-muted",
          )}
        >
          {logoProvider ? (
            <ProviderLogo provider={logoProvider} className="size-4 shrink-0" />
          ) : (
            <Cpu className="size-4 shrink-0" aria-hidden />
          )}
          <span className="min-w-0 truncate">
            {t(($) => $.model_dropdown.managed_by_runtime_title)}
          </span>
        </div>
      ) : (
        <AIModelSelector open={open} onOpenChange={setOpen}>
          <AIModelSelectorTrigger
            disabled={disabled}
            aria-label={t(($) => $.pickers.model_tooltip, {
              value: triggerLabel,
            })}
            className={triggerClassName(variant, showLabel)}
          >
            {trigger}
          </AIModelSelectorTrigger>
          <AIModelSelectorContent
            command={false}
            title={t(($) => $.model_dropdown.label)}
            className="w-[min(48rem,calc(100vw-2rem))] max-w-[min(48rem,calc(100vw-2rem))] gap-0 overflow-hidden p-0 sm:max-w-[min(48rem,calc(100vw-2rem))]"
          >
            {open && (
              <ModelSelectorContent
                key={runtimeId}
                runtimes={choices}
                runtimeId={runtimeId ?? ""}
                model={value}
                thinkingLevel={thinkingLevel}
                serviceTier={serviceTier}
                allowEffort={effortEditable}
                allowSpeed={speedEditable}
                onSelect={async (selection) => {
                  if (onSelection) await onSelection(selection);
                  else await onChange(selection.model);
                  setOpen(false);
                }}
              />
            )}
          </AIModelSelectorContent>
        </AIModelSelector>
      )}
    </div>
  );
}

function TriggerMeta({ children }: { children: ReactNode }) {
  return (
    <>
      <span className="h-3 w-px shrink-0 bg-border" aria-hidden />
      <span className="shrink-0 text-muted-foreground">{children}</span>
    </>
  );
}

function triggerClassName(variant: "field" | "chip", showLabel: boolean) {
  return cn(
    "inline-flex max-w-full cursor-pointer items-center gap-1.5 rounded-full bg-muted text-left text-foreground transition-colors hover:bg-accent focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring active:bg-accent data-open:bg-accent data-open:text-accent-foreground data-popup-open:bg-accent data-popup-open:text-accent-foreground disabled:pointer-events-none disabled:opacity-50 disabled:hover:bg-muted",
    showLabel && "mt-1.5",
    variant === "field" ? "h-8 px-2.5 text-caption" : "h-7 px-2 text-micro",
  );
}
