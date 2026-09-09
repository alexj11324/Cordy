import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import {
  ShellHeaderActions,
  ShellHeaderActionsSlot,
  ShellHeaderProvider,
} from "./shell-header";

describe("ShellHeaderActions", () => {
  it("portals page actions into the shell slot", () => {
    render(
      <ShellHeaderProvider>
        <header>
          <span>crumb</span>
          <ShellHeaderActionsSlot />
        </header>
        <ShellHeaderActions>
          <button type="button">New project</button>
        </ShellHeaderActions>
      </ShellHeaderProvider>,
    );

    const slot = document.querySelector("[data-slot='shell-header-actions']")!;
    expect(slot).toContainElement(
      screen.getByRole("button", { name: "New project" }),
    );
  });

  it("renders actions in place when the shell slot is not mounted", () => {
    render(
      <ShellHeaderActions>
        <button type="button">New project</button>
      </ShellHeaderActions>,
    );

    expect(
      screen.getByRole("button", { name: "New project" }),
    ).toBeInTheDocument();
  });
});
