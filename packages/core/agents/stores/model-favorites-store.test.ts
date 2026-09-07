// @vitest-environment jsdom
import { beforeEach, describe, expect, it } from "vitest";
import {
  modelFavoriteKey,
  useModelFavoritesStore,
} from "./model-favorites-store";

describe("model favorites", () => {
  beforeEach(() => useModelFavoritesStore.setState({ favorites: [] }));
  it("treats missing serviceTier as empty so older persisted favorites still match", () => {
    expect(
      modelFavoriteKey({
        runtimeId: "codex",
        model: "gpt",
        thinkingLevel: "low",
      }),
    ).toBe(
      modelFavoriteKey({
        runtimeId: "codex",
        model: "gpt",
        thinkingLevel: "low",
        serviceTier: "",
      }),
    );
  });
  it("keeps speed combinations distinct from effort-only favorites", () => {
    const standard = {
      runtimeId: "codex",
      model: "gpt",
      thinkingLevel: "low",
      serviceTier: "default",
    };
    const fast = { ...standard, serviceTier: "priority" };
    [standard, fast].forEach(useModelFavoritesStore.getState().toggle);
    expect(useModelFavoritesStore.getState().favorites).toEqual([
      standard,
      fast,
    ]);
  });
  it("keeps efforts and runtime accounts distinct and removes only the matching combination", () => {
    const high = {
      runtimeId: "codex-personal",
      model: "gpt",
      thinkingLevel: "high",
    };
    const low = { ...high, thinkingLevel: "low" };
    const work = { ...high, runtimeId: "codex-work" };
    [high, low, work].forEach(useModelFavoritesStore.getState().toggle);
    useModelFavoritesStore.getState().toggle(high);
    expect(useModelFavoritesStore.getState().favorites).toEqual([low, work]);
    expect(modelFavoriteKey(high)).not.toBe(modelFavoriteKey(low));
  });
  it("restores exact model and effort combinations from persisted storage", async () => {
    const choice = { runtimeId: "codex", model: "gpt", thinkingLevel: "xhigh" };
    useModelFavoritesStore.getState().toggle(choice);
    const persisted = localStorage.getItem("orvilo_agent_model_favorites")!;
    useModelFavoritesStore.setState({ favorites: [] });
    localStorage.setItem("orvilo_agent_model_favorites", persisted);
    await useModelFavoritesStore.persist.rehydrate();
    expect(useModelFavoritesStore.getState().favorites).toEqual([choice]);
  });
});
