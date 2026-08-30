// @vitest-environment jsdom

import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import type { Agent } from "@patchbay/core/types";
import { I18nProvider } from "@patchbay/core/i18n/react";
import enCommon from "../../../locales/en/common.json";
import enAgents from "../../../locales/en/agents.json";
import { InstructionsTab } from "./instructions-tab";

const TEST_RESOURCES = { en: { common: enCommon, agents: enAgents } };

const agent = {
  id: "agent-1",
  instructions: "Keep the preview read-only.",
  system_instructions: null,
} as unknown as Agent;

function renderTab(canEdit: boolean) {
  return render(
    <I18nProvider locale="en" resources={TEST_RESOURCES}>
      <InstructionsTab
        agent={agent}
        canEdit={canEdit}
        onSave={vi.fn().mockResolvedValue(undefined)}
      />
    </I18nProvider>,
  );
}

describe("InstructionsTab read-only mode", () => {
  it("disables the workspace prompt editor and save action", () => {
    renderTab(false);

    expect(screen.getByRole("textbox")).toBeDisabled();
    expect(screen.getByRole("button", { name: "Save" })).toBeDisabled();
  });
});
