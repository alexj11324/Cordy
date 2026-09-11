"use client";

import { Check } from "lucide-react";
import type { RuntimeModelServiceTier } from "@orvilo/core/types";
import { cn } from "@orvilo/ui/lib/utils";
import { useT } from "../../i18n";

/** Speed choices use the same catalog as the model and effort columns. */
export function ModelSpeedColumn({
  tiers,
  supportsExplicitStandard,
  value,
  editable,
  disabled,
  onSelect,
}: {
  tiers: RuntimeModelServiceTier[];
  supportsExplicitStandard: boolean;
  value: string;
  editable: boolean;
  disabled: boolean;
  onSelect: (value: string) => void;
}) {
  const { t } = useT("agents");
  const available = supportsExplicitStandard
    ? [
        {
          id: "default",
          name: t(($) => $.pickers.service_tier_standard),
          description: t(($) => $.pickers.service_tier_standard_description),
        },
        ...tiers.filter((tier) => tier.id !== "default"),
      ]
    : tiers;
  const selected = available.find((tier) => tier.id === value);
  const inheritedLabel = value
    ? (selected?.name ?? value)
    : t(($) => $.model_selector.speed_inherited);

  return (
    <div
      aria-label={t(($) => $.inspector.prop_speed)}
      className="min-w-[10.5rem] overflow-y-auto border-l border-border/60 p-1.5"
    >
      <p className="px-2 py-1 text-micro text-muted-foreground">
        {t(($) => $.inspector.prop_speed)}
      </p>
      {!editable ? (
        <div className="px-2 py-5 text-caption text-muted-foreground">
          <p>{inheritedLabel}</p>
          <p className="mt-2">{t(($) => $.model_selector.speed_inherited)}</p>
        </div>
      ) : available.length === 0 && !value ? (
        <p className="px-2 py-5 text-caption text-muted-foreground">
          {t(($) => $.model_selector.no_speed)}
        </p>
      ) : (
        <>
          {value && !selected && (
            <button
              type="button"
              disabled={disabled}
              aria-label={t(($) => $.model_selector.clear_unsupported_speed, {
                value,
              })}
              onClick={() => onSelect("")}
              className="flex w-full min-w-0 items-center gap-2 rounded-md px-2 py-2.5 text-left text-caption hover:bg-accent/60 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
            >
              <span className="min-w-0 flex-1 break-words">{value}</span>
              <Check aria-hidden className="size-4 shrink-0" />
            </button>
          )}
          {available.map((tier) => (
            <button
              key={tier.id}
              type="button"
              disabled={disabled}
              aria-pressed={value === tier.id}
              title={tier.description}
              onClick={() => onSelect(tier.id)}
              className={cn(
                "flex w-full min-w-0 items-center gap-2 rounded-md px-2 py-2.5 text-left text-caption hover:bg-accent/60 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
                value === tier.id &&
                  "bg-accent text-accent-foreground font-medium ring-1 ring-inset ring-border",
              )}
            >
              <span className="min-w-0 flex-1 break-words">{tier.name}</span>
              {value === tier.id && (
                <Check aria-hidden className="size-4 shrink-0" />
              )}
            </button>
          ))}
        </>
      )}
    </div>
  );
}
