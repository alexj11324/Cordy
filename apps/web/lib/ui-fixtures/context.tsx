"use client";

import { createContext, useContext, type ReactNode } from "react";

const UiFixturesContext = createContext(false);

export function UiFixturesProvider({
  enabled,
  children,
}: {
  enabled: boolean;
  children: ReactNode;
}) {
  return (
    <UiFixturesContext.Provider value={enabled}>
      {children}
    </UiFixturesContext.Provider>
  );
}

export function useUiFixtures(): boolean {
  return useContext(UiFixturesContext);
}
