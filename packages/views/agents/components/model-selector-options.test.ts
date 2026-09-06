// @vitest-environment node
import { describe, expect, it } from "vitest";
import { groupModelSelectorOptions } from "./model-selector-options";
const models = [
  { id: "gemini-flash-high", label: "Gemini Flash (High)" },
  { id: "gemini-flash-medium", label: "Gemini Flash (Medium)" },
  { id: "gemini-flash-low", label: "Gemini Flash (Low)" },
  { id: "other", label: "Other" },
];
describe("Antigravity native effort variants", () => {
  it("groups discovered variants while preserving every exact runtime ID", () => {
    const options = groupModelSelectorOptions(models, "antigravity");
    expect(options).toHaveLength(2);
    expect(options[0]?.label).toBe("Gemini Flash");
    expect(options[0]?.variants?.map((model) => model.id)).toEqual(
      models.slice(0, 3).map((model) => model.id),
    );
  });
  it("does not infer variants for other providers, singletons, or unmatched native IDs", () => {
    expect(groupModelSelectorOptions(models, "codex")).toEqual(models);
    expect(groupModelSelectorOptions([models[0]!], "antigravity")).toEqual([
      models[0],
    ]);
    const unmatched = [
      { id: "high", label: "Gemini Flash (High)" },
      { id: "low", label: "Gemini Flash (Low)" },
    ];
    expect(groupModelSelectorOptions(unmatched, "antigravity")).toEqual(
      unmatched,
    );
  });
});
