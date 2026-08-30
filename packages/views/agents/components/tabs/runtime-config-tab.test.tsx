// @vitest-environment jsdom

import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import type { Agent } from "@patchbay/core/types";
import { I18nProvider } from "@patchbay/core/i18n/react";
import enCommon from "../../../locales/en/common.json";
import enAgents from "../../../locales/en/agents.json";
import { RuntimeConfigTab } from "./runtime-config-tab";

const TEST_RESOURCES = { en: { common: enCommon, agents: enAgents } };

const agent = {
  id: "agent-1",
  runtime_config: {
    mode: "gateway",
    gateway: { host: "127.0.0.1", port: 18789, token: "secret", tls: true },
  },
} as unknown as Agent;

function renderTab(canEdit: boolean) {
  return render(
    <I18nProvider locale="en" resources={TEST_RESOURCES}>
      <RuntimeConfigTab
        agent={agent}
        canEdit={canEdit}
        onSave={vi.fn().mockResolvedValue(undefined)}
      />
    </I18nProvider>,
  );
}

describe("RuntimeConfigTab read-only mode", () => {
  it("disables routing mode, gateway fields, and save", () => {
    renderTab(false);

    expect(screen.getByRole("button", { name: "Local" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Gateway" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Save" })).toBeDisabled();
    expect(screen.getByRole("textbox", { name: "Host" })).toBeDisabled();
    expect(screen.getByRole("textbox", { name: "Port" })).toBeDisabled();
    expect(screen.getByLabelText("Auth token")).toBeDisabled();
    expect(screen.getByRole("switch")).toHaveAttribute("aria-disabled", "true");
  });
});
