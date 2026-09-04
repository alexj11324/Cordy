// @vitest-environment jsdom

import { render, screen } from "@testing-library/react";
import "@testing-library/jest-dom/vitest";
import { describe, expect, it } from "vitest";
import { AuthShell } from "./auth-shell";

describe("AuthShell", () => {
  it("renders the Accounts login as white-left and black-right panels", () => {
    render(
      <AuthShell>
        <div data-testid="clerk-form" />
      </AuthShell>,
    );

    expect(screen.getByTestId("accounts-auth-shell")).toHaveClass(
      "accounts-auth-shell",
    );
    expect(screen.getByTestId("accounts-auth-form-panel")).toContainElement(
      screen.getByTestId("clerk-form"),
    );
    expect(screen.getByTestId("accounts-auth-brand-panel")).toHaveClass(
      "accounts-auth-brand-panel",
    );
    expect(screen.getByTestId("patchbay-mark")).toHaveAttribute(
      "src",
      "/icons/icon.svg",
    );
  });
});
