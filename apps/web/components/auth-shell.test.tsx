import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { AuthShell } from "./auth-shell";

vi.mock("@orvilo/ui/components/common/orvilo-icon", () => ({
  OrviloIcon: () => <div data-testid="orvilo-icon" />,
}));

describe("AuthShell", () => {
  it("renders the custom dark shadcn form and brand layout", () => {
    render(
      <AuthShell>
        <div data-testid="auth-form" />
      </AuthShell>,
    );

    expect(screen.getByTestId("auth-shell")).toHaveClass(
      "dark",
      "bg-background",
      "md:grid-cols-2",
    );
    expect(screen.getByTestId("auth-form").parentElement).toHaveClass(
      "bg-background",
    );
    expect(screen.getByTestId("auth-brand-panel")).toHaveClass(
      "bg-card",
      "md:flex",
    );
    expect(screen.getByTestId("orvilo-icon")).toBeInTheDocument();
  });
});
