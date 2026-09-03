// @vitest-environment jsdom

import { afterEach, describe, expect, it } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";
import "@testing-library/jest-dom/vitest";
import { I18nProvider } from "@patchbay/core/i18n/react";
import enSettings from "../../locales/en/settings.json";
import zhSettings from "../../locales/zh-Hans/settings.json";
import { MessagingConnectionStatus } from "./messaging-connection-status";

afterEach(cleanup);

describe("MessagingConnectionStatus", () => {
  it("renders the requested connection terminology with an accessible state", () => {
    render(
      <I18nProvider
        locale="zh-Hans"
        resources={{ "zh-Hans": { settings: zhSettings } }}
      >
        <MessagingConnectionStatus
          installation={{
            status: "installed",
            runtime: {
              state: "healthy",
              observedAt: "2026-09-03T10:00:00Z",
              errorCode: null,
            },
          }}
        />
      </I18nProvider>,
    );
    expect(screen.getByRole("status", { name: "连接状态" })).toHaveTextContent(
      "已连接",
    );
    expect(screen.queryByText("健康")).toBeNull();
  });

  it("shows missing observations as unknown and never renders provider diagnostics", () => {
    render(
      <I18nProvider locale="en" resources={{ en: { settings: enSettings } }}>
        <MessagingConnectionStatus
          installation={{
            status: "installed",
            runtime: {
              state: "future_state",
              observedAt: null,
              errorCode: "future_code",
              errorSummary: "credential-sentinel",
            },
          }}
        />
      </I18nProvider>,
    );
    expect(
      screen.getByRole("status", { name: "Connection status" }),
    ).toHaveTextContent("Status unavailable");
    expect(screen.queryByText("credential-sentinel")).toBeNull();
  });
});
