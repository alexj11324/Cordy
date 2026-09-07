/** @vitest-environment jsdom */

import { cleanup, render, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { setApiInstance } from "../api";
import { createAuthStore, registerAuthStore } from "../auth";
import type { ApiClient } from "../api/client";
import { I18nProvider, LocaleAdapterProvider } from "./react";
import { UserLocaleSync } from "./user-locale-sync";
import type { LocaleAdapter } from "./types";
import type { User } from "../types";

afterEach(cleanup);

function makeAdapter(persist: LocaleAdapter["persist"]): LocaleAdapter {
  return {
    getUserChoice: () => null,
    getSystemPreferences: () => [],
    persist,
  };
}

describe("UserLocaleSync", () => {
  it("normalizes a removed server locale to English", async () => {
    const updateMe = vi.fn().mockResolvedValue({ language: "en" });
    const api = { updateMe } as unknown as ApiClient;
    setApiInstance(api);
    const store = createAuthStore({
      api,
      storage: {
        getItem: () => null,
        setItem: () => {},
        removeItem: () => {},
      },
    });
    store.setState({
      user: { id: "user-1", language: "ko" } as User,
    });
    registerAuthStore(store);
    const persist = vi.fn();

    render(
      <I18nProvider locale="en" resources={{ en: {} }}>
        <LocaleAdapterProvider adapter={makeAdapter(persist)}>
          <UserLocaleSync />
        </LocaleAdapterProvider>
      </I18nProvider>,
    );

    await waitFor(() => {
      expect(persist).toHaveBeenCalledWith("en");
      expect(updateMe).toHaveBeenCalledWith({ language: "en" });
    });
  });
});
