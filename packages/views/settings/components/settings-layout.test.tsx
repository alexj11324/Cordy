// @vitest-environment jsdom

import { describe, expect, it } from "vitest";
import { render, screen } from "@testing-library/react";
import { Check, Pencil } from "lucide-react";
import {
  SETTINGS_CONTROL_CLASS,
  SettingsCard,
  SettingsPillButton,
  SettingsRow,
  SettingsSearchField,
  SettingsSection,
  SettingsTab,
} from "./settings-layout";

describe("settings layout primitives", () => {
  it("renders the page title as a large heading with supporting copy", () => {
    render(
      <SettingsTab title="Profile" description="How you appear.">
        <div>body</div>
      </SettingsTab>,
    );

    const heading = screen.getByRole("heading", { level: 2, name: "Profile" });
    expect(heading).toHaveClass("text-display-sm");
    expect(screen.getByText("How you appear.")).toHaveClass("text-title-sm");
  });

  it("renders section titles in muted body type, not as a second page title", () => {
    render(
      <SettingsSection title="Profile info">
        <SettingsCard>
          <div>row</div>
        </SettingsCard>
      </SettingsSection>,
    );

    const heading = screen.getByRole("heading", {
      level: 3,
      name: "Profile info",
    });
    expect(heading).toHaveClass("text-muted-foreground");
    expect(
      screen.getByText("row").closest("[data-slot=settings-section-card]"),
    ).toHaveClass("rounded-xl");
  });

  it("stacks a field's value under its label", () => {
    render(
      <SettingsRow layout="stack" label="Display name">
        <p>Ada</p>
      </SettingsRow>,
    );

    const label = screen.getByText("Display name");
    const value = screen.getByText("Ada");
    expect(label.compareDocumentPosition(value) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(value.parentElement).toBe(label.parentElement);
  });

  it("keeps split rows as label-then-control siblings", () => {
    render(
      <SettingsRow label="Theme">
        <button type="button">Dark</button>
      </SettingsRow>,
    );

    const label = screen.getByText("Theme");
    const control = screen.getByRole("button", { name: "Dark" });
    expect(label.parentElement).not.toBe(control.parentElement);
  });

  it("renders an inactive pill and an active done pill", () => {
    const { rerender } = render(
      <SettingsPillButton icon={Pencil}>Edit</SettingsPillButton>,
    );

    expect(screen.getByRole("button", { name: "Edit" })).toHaveClass(
      "rounded-full",
    );

    rerender(
      <SettingsPillButton icon={Check} active>
        Done
      </SettingsPillButton>,
    );

    expect(screen.getByRole("button", { name: "Done" })).toHaveClass(
      "bg-primary",
    );
  });

  it("renders a destructive pill without Linear outline chrome", () => {
    render(
      <SettingsPillButton tone="destructive">Leave</SettingsPillButton>,
    );

    expect(screen.getByRole("button", { name: "Leave" })).toHaveClass(
      "rounded-full",
      "bg-destructive/10",
    );
  });

  it("renders search as a filled pill, not a bordered Linear field", () => {
    render(
      <SettingsSearchField
        value=""
        onValueChange={() => undefined}
        placeholder="Search actions..."
      />,
    );

    expect(screen.getByLabelText("Search actions...")).toHaveClass(
      "rounded-full",
    );
    expect(SETTINGS_CONTROL_CLASS).toContain("rounded-full");
  });
});
