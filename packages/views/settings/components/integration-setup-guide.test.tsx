// @vitest-environment jsdom

import { afterEach, describe, expect, it } from "vitest";
import "@testing-library/jest-dom/vitest";
import { cleanup, render, screen } from "@testing-library/react";
import { I18nProvider } from "@patchbay/core/i18n/react";
import enCommon from "../../locales/en/common.json";
import enSettings from "../../locales/en/settings.json";
import { IntegrationSetupGuide } from "./integration-setup-guide";

afterEach(cleanup);

const channels = ["lark", "slack", "dingtalk", "wecom", "telegram", "weixin"] as const;

describe("IntegrationSetupGuide", () => {
  it.each(channels)("keeps the complete %s setup checklist on the integrations page", (channel) => {
    render(
      <I18nProvider locale="en" resources={{ en: { common: enCommon, settings: enSettings } }}>
        <IntegrationSetupGuide channel={channel} />
      </I18nProvider>,
    );

    const guide = screen.getByTestId(`integration-setup-guide-${channel}`);
    expect(guide).toHaveTextContent("What you need");
    expect(guide).toHaveTextContent("Complete these steps");
    expect(guide.querySelectorAll("ol > li")).toHaveLength(3);
  });

  it("links the Slack manifest instructions before the app dashboard", () => {
    render(
      <I18nProvider locale="en" resources={{ en: { common: enCommon, settings: enSettings } }}>
        <IntegrationSetupGuide channel="slack" />
      </I18nProvider>,
    );

    expect(screen.getByRole("button", { name: "View Patchbay manifest" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Open Slack app dashboard" })).toBeInTheDocument();
  });

  it("keeps hosted Slack setup inside the managed OAuth flow", () => {
    render(
      <I18nProvider locale="en" resources={{ en: { common: enCommon, settings: enSettings } }}>
        <IntegrationSetupGuide channel="slack" managed />
      </I18nProvider>,
    );

    expect(screen.getByText(/Click Connect Slack on this page/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "View Patchbay manifest" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Open Slack app dashboard" })).not.toBeInTheDocument();
  });
});
