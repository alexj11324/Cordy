// @vitest-environment jsdom
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import {
  SettingsBackButton,
  SettingsInput,
  SettingsSwitch,
  SettingsText,
} from "@patchbay/ui/components/common/lobe-settings";
import { LobeSettingsProvider } from "@patchbay/ui/components/common/lobe-settings-provider";

describe("Lobe settings adapters", () => {
  it("uses Lobe controls without changing their settings semantics", async () => {
    const onCheckedChange = vi.fn();
    const onInputChange = vi.fn();

    render(
      <LobeSettingsProvider>
        <SettingsText as="h2">Profile</SettingsText>
        <SettingsBackButton
          data-settings-initial-focus
          onClick={() => undefined}
        >
          Back to app
        </SettingsBackButton>
        <SettingsSwitch
          aria-label="Enable floating chat"
          checked={false}
          onCheckedChange={onCheckedChange}
        />
        <SettingsInput
          aria-label="Display name"
          value="Cordy"
          onChange={onInputChange}
        />
      </LobeSettingsProvider>,
    );

    await waitFor(() => {
      expect(
        document.querySelector('[data-settings-ui="lobe-runtime"]'),
      ).toBeInTheDocument();
    });
    expect(screen.getByRole("heading", { name: "Profile" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Back to app" })).toHaveAttribute(
      "data-settings-initial-focus",
    );
    const toggle = screen.getByRole("switch", { name: "Enable floating chat" });
    expect(toggle).toBeInTheDocument();
    fireEvent.click(toggle);
    expect(onCheckedChange).toHaveBeenCalledWith(true);

    const input = screen.getByRole("textbox", { name: "Display name" });
    expect(input).toHaveValue("Cordy");
    fireEvent.change(input, { target: { value: "Cordy Prime" } });
    expect(onInputChange).toHaveBeenCalled();
  });
});
