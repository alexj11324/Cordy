"use client";

import { create } from "zustand";
import { createJSONStorage, persist } from "zustand/middleware";
import { defaultStorage } from "../../platform/storage";

/** Runtime identity distinguishes separate accounts/installations of one provider. */
export interface ModelFavorite {
  runtimeId: string;
  model: string;
  thinkingLevel: string;
  /** Display snapshots for favorites whose runtime catalog is not loaded yet. */
  modelLabel?: string;
  thinkingLabel?: string;
}

export function modelFavoriteKey(choice: ModelFavorite): string {
  return JSON.stringify([choice.runtimeId, choice.model, choice.thinkingLevel]);
}

export const useModelFavoritesStore = create<{
  favorites: ModelFavorite[];
  toggle: (choice: ModelFavorite) => void;
}>()(
  persist(
    (set) => ({
      favorites: [],
      toggle: (choice) =>
        set(({ favorites }) => {
          const key = modelFavoriteKey(choice);
          return {
            favorites: favorites.some((item) => modelFavoriteKey(item) === key)
              ? favorites.filter((item) => modelFavoriteKey(item) !== key)
              : [...favorites, choice],
          };
        }),
    }),
    {
      name: "orvilo_agent_model_favorites",
      storage: createJSONStorage(() => defaultStorage),
      partialize: ({ favorites }) => ({ favorites }),
    },
  ),
);
