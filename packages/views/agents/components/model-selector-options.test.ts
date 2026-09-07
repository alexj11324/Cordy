// @vitest-environment node
import { describe, expect, it } from "vitest";
import {
  groupModelSelectorOptions,
  serviceTierDisplayName,
  thinkingLevelDisplayName,
} from "./model-selector-options";
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

describe("catalog display names", () => {
  const opus = {
    id: "claude-opus-5",
    label: "Claude Opus 5",
    supports_explicit_standard_service_tier: true,
    service_tiers: [{ id: "true", name: "Fast" }],
    thinking: {
      supported_levels: [{ value: "low", label: "Low" }],
    },
  };

  it("maps Claude Fast's stored id to the catalog name", () => {
    expect(serviceTierDisplayName("true", opus, "Standard")).toBe("Fast");
  });

  it("maps explicit standard without looking up a catalog row", () => {
    expect(serviceTierDisplayName("default", undefined, "Standard")).toBe(
      "Standard",
    );
  });

  it("keeps an unknown stored id visible instead of inventing a label", () => {
    expect(serviceTierDisplayName("priority", opus, "Standard")).toBe(
      "priority",
    );
  });

  it("maps a thinking-level id to the catalog label", () => {
    expect(thinkingLevelDisplayName("low", opus)).toBe("Low");
    expect(thinkingLevelDisplayName("low", undefined)).toBe("low");
  });
});
