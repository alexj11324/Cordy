// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { I18nProvider } from "@patchbay/core/i18n/react";
import enAgents from "../../locales/en/agents.json";
import { ModelSpeedColumn } from "./model-speed-column";

const fast = { id: "priority", name: "Fast", description: "Faster responses" };
function mount(
  props: Partial<React.ComponentProps<typeof ModelSpeedColumn>> = {},
) {
  const onSelect = vi.fn();
  render(
    <I18nProvider locale="en" resources={{ en: { agents: enAgents } }}>
      <ModelSpeedColumn
        tiers={[fast]}
        supportsExplicitStandard={false}
        value=""
        editable
        disabled={false}
        onSelect={onSelect}
        {...props}
      />
    </I18nProvider>,
  );
  return onSelect;
}
afterEach(cleanup);
describe("ModelSpeedColumn", () => {
  it("preserves the native tier ID and provider description", () => {
    const onSelect = mount();
    fireEvent.click(screen.getByTitle(fast.description));
    expect(onSelect).toHaveBeenCalledWith("priority");
  });
  it("does not invent explicit Standard on an older daemon", () => {
    mount();
    expect(screen.queryByRole("button", { name: "Standard" })).toBeNull();
  });
  it("offers CLI-level Standard even without alternative tiers", () => {
    const onSelect = mount({ tiers: [], supportsExplicitStandard: true });
    fireEvent.click(screen.getByRole("button", { name: "Standard" }));
    expect(onSelect).toHaveBeenCalledWith("default");
  });
  it("keeps an unknown saved tier visible but only lets the user clear it", () => {
    const onSelect = mount({ tiers: [], value: "retired-fast" });
    expect(screen.getByText("retired-fast")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "retired-fast" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Remove unsupported speed retired-fast" }));
    expect(onSelect).toHaveBeenCalledWith("");
  });
  it("shows an unavailable column without accepting fabricated speed values", () => {
    mount({ tiers: [] });
    expect(screen.getByText(enAgents.model_selector.no_speed)).toBeTruthy();
    expect(screen.queryByRole("button")).toBeNull();
  });
  it("prevents changes while the combined selection is saving", () => {
    const onSelect = mount({ disabled: true });
    fireEvent.click(screen.getByRole("button", { name: "Fast" }));
    expect(onSelect).not.toHaveBeenCalled();
  });
});
