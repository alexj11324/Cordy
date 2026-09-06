import { act, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { renderWithI18n } from "../../test/i18n";
import { TriggerAddMenu } from "./trigger-add-menu";

describe("TriggerAddMenu", () => {
  it("opens the GitHub event submenu without crashing", async () => {
    const user = userEvent.setup();

    renderWithI18n(
      <TriggerAddMenu
        canWrite
        onPickSchedule={vi.fn()}
        onPickPreset={vi.fn()}
      />,
    );

    await user.click(screen.getByRole("button", { name: "Add Trigger" }));
    const github = await screen.findByRole("menuitem", { name: "GitHub" });
    await act(async () => {
      github.focus();
    });
    await user.keyboard("{ArrowRight}");

    expect(
      screen.getByRole("menuitem", { name: "PR opened" }),
    ).toBeInTheDocument();
  });
});
