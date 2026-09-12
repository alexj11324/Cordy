import { screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { renderWithI18n } from "../../test/i18n";
import { SettingsFormRow, SettingsGroup } from "./settings-shell";

// `lobe: true` mounts the theme bridge; the first query therefore has to be an
// async one (`findBy`), because the bridge is loaded on demand.

describe("SettingsGroup", () => {
  it("renders the section header, its action and its rows", async () => {
    renderWithI18n(
      <SettingsGroup
        title="Appearance"
        description="How Orvilo looks"
        extra={<button type="button">Reset</button>}
      >
        <div>Row body</div>
      </SettingsGroup>,
      { lobe: true },
    );

    expect(await screen.findByText("Appearance")).toBeTruthy();
    expect(screen.getByText("How Orvilo looks")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Reset" })).toBeTruthy();
    expect(screen.getByText("Row body")).toBeTruthy();
  });

  // `Form.Group` derives `collapsible` from the variant when it is undefined,
  // and an outlined group is collapsible by default. That turns the section
  // title into a disclosure toggle, which both hides rows on a stray click and
  // breaks the settings nav search — it matches on row content, and a collapsed
  // section matches nothing. The title must not be a button.
  it("does not make the section title a collapse toggle", async () => {
    renderWithI18n(
      <SettingsGroup title="Appearance">
        <div>Row body</div>
      </SettingsGroup>,
      { lobe: true },
    );

    expect(await screen.findByText("Appearance")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Appearance" })).toBeNull();
  });
});

describe("SettingsFormRow", () => {
  it("renders its label, description and control", async () => {
    renderWithI18n(
      <SettingsFormRow label="Theme" description="Applies to this device">
        <span>control</span>
      </SettingsFormRow>,
      { lobe: true },
    );

    expect(await screen.findByText("Theme")).toBeTruthy();
    expect(screen.getByText("Applies to this device")).toBeTruthy();
    expect(screen.getByText("control")).toBeTruthy();
  });

  it("reserves the control column from minWidth", async () => {
    const { container } = renderWithI18n(
      <SettingsFormRow label="Theme" minWidth={384}>
        <span>control</span>
      </SettingsFormRow>,
      { lobe: true },
    );

    await screen.findByText("Theme");

    const row = container.querySelector<HTMLElement>(
      "[style*='--form-item-min-width']",
    );
    expect(row?.style.getPropertyValue("--form-item-min-width")).toBe("384px");
  });

  it("draws a separator above a row only when asked", async () => {
    renderWithI18n(
      <>
        <SettingsFormRow label="First">
          <span>first control</span>
        </SettingsFormRow>
        <SettingsFormRow label="Second" divider>
          <span>second control</span>
        </SettingsFormRow>
      </>,
      { lobe: true },
    );

    await screen.findByText("First");
    expect(screen.getAllByRole("separator")).toHaveLength(1);
  });
});
