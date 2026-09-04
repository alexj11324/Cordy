// @vitest-environment jsdom

import { render, screen } from "@testing-library/react";
import "@testing-library/jest-dom/vitest";
import { describe, expect, it } from "vitest";
import { AuthShell } from "./auth-shell";

describe("AuthShell", () => {
  it("renders the Accounts login in the shadcn charcoal-left, black-right layout", () => {
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
    expect(screen.getByTestId("accounts-auth-form-panel")).toHaveClass(
      "accounts-auth-form-panel--right",
    );
    expect(screen.getByTestId("accounts-auth-brand-panel")).toHaveClass(
      "accounts-auth-brand-panel",
    );
    expect(screen.getByTestId("accounts-auth-brand-panel")).toHaveClass(
      "accounts-auth-brand-panel--left",
    );
    expect(screen.getByTestId("accounts-auth-brand-panel")).toHaveAttribute(
      "data-panel-tone",
      "charcoal",
    );
    expect(screen.getByTestId("accounts-auth-form-panel")).toHaveAttribute(
      "data-panel-tone",
      "black",
    );
    expect(screen.getByTestId("accounts-auth-shell").firstElementChild).toBe(
      screen.getByTestId("accounts-auth-brand-panel"),
    );
    expect(screen.getByTestId("accounts-auth-shell").lastElementChild).toBe(
      screen.getByTestId("accounts-auth-form-panel"),
    );
    expect(screen.getByText("Login")).toBeInTheDocument();
    expect(screen.getByText(/Sofia Davis/)).toBeInTheDocument();
    expect(screen.getByTestId("patchbay-mark")).toHaveAttribute(
      "src",
      "/icons/icon.svg",
    );
  });
});
