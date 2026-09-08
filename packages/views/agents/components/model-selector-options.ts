import type { RuntimeModel } from "@orvilo/core/types";

export type ModelSelectorOption = RuntimeModel & { variants?: RuntimeModel[] };

/** Antigravity encodes effort in native model IDs instead of a separate flag. */
export function groupModelSelectorOptions(
  models: RuntimeModel[],
  provider: string,
): ModelSelectorOption[] {
  if (provider !== "antigravity") return models;
  const groups = new Map<string, RuntimeModel[]>();
  for (const model of models) {
    const match = model.label.match(/^(.*) \((High|Medium|Low)\)$/);
    if (
      !match ||
      !model.id.endsWith(`-${match[2]!.toLowerCase()}`) ||
      model.thinking?.supported_levels.length
    )
      continue;
    const key = `${model.id.replace(/-(high|medium|low)$/, "")}\0${match[1]}`;
    groups.set(key, [...(groups.get(key) ?? []), model]);
  }
  const grouped = new Map<string, ModelSelectorOption>();
  for (const variants of groups.values()) {
    if (variants.length < 2) continue;
    const first = variants[0]!;
    grouped.set(first.id, {
      ...first,
      label: first.label.replace(/ \((High|Medium|Low)\)$/, ""),
      variants,
    });
  }
  const nestedIds = new Set(
    [...grouped.values()].flatMap((group) =>
      group.variants!.slice(1).map((variant) => variant.id),
    ),
  );
  return models
    .filter((model) => !nestedIds.has(model.id))
    .map((model) => grouped.get(model.id) ?? model);
}

export function nativeEffortLabel(model: RuntimeModel): string {
  return model.label.match(/\((High|Medium|Low)\)$/)?.[1] ?? model.label;
}

/** Persist a real catalog level instead of an empty "follow CLI" sentinel. */
export function explicitThinkingLevel(
  entry: RuntimeModel | null | undefined,
  current: string,
): string {
  const levels = entry?.thinking?.supported_levels ?? [];
  if (current && levels.some((level) => level.value === current)) return current;
  const fallback = entry?.thinking?.default_level;
  if (fallback && levels.some((level) => level.value === fallback)) {
    return fallback;
  }
  return levels[0]?.value ?? "";
}

/** Persist a real speed tier instead of an empty "runtime default" sentinel. */
export function explicitServiceTier(
  entry: RuntimeModel | null | undefined,
  current: string,
): string {
  const tiers = entry?.service_tiers ?? [];
  if (
    current &&
    (current === "default"
      ? entry?.supports_explicit_standard_service_tier === true
      : tiers.some((tier) => tier.id === current))
  ) {
    return current;
  }
  if (entry?.supports_explicit_standard_service_tier === true) return "default";
  return tiers.find((tier) => tier.id !== "default")?.id ?? tiers[0]?.id ?? "";
}

/** Resolve a stored service-tier id to the catalog name the user picked. */
export function serviceTierDisplayName(
  serviceTier: string,
  entry: RuntimeModel | null | undefined,
  standardLabel: string,
): string {
  if (!serviceTier) return "";
  if (serviceTier === "default") return standardLabel;
  return (
    entry?.service_tiers?.find((tier) => tier.id === serviceTier)?.name ??
    serviceTier
  );
}

/** Resolve a stored thinking-level id to the catalog label. */
export function thinkingLevelDisplayName(
  thinkingLevel: string,
  entry: RuntimeModel | null | undefined,
): string {
  if (!thinkingLevel) return "";
  return (
    entry?.thinking?.supported_levels.find(
      (level) => level.value === thinkingLevel,
    )?.label ?? thinkingLevel
  );
}
