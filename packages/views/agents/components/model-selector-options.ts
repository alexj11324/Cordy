import type { RuntimeModel } from "@patchbay/core/types";

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
