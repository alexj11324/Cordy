import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { AuthShell } from "./auth-shell";

vi.mock("@patchbay/ui/components/common/patchbay-icon", () => ({
  PatchbayIcon: () => <div data-testid="patchbay-icon" />,
}));

describe("AuthShell", () => {
  it("renders the custom dark shadcn form and brand layout", () => {
    render(
      <AuthShell>
        <div data-testid="auth-form" />
      </AuthShell>,
    );

    expect(screen.getByTestId("auth-shell")).toHaveClass(
      "bg-zinc-950",
      "md:grid-cols-2",
    );
    expect(screen.getByTestId("auth-form").parentElement).toHaveClass(
      "bg-zinc-950",
    );
    expect(screen.getByTestId("auth-brand-panel")).toHaveClass(
      "bg-zinc-950",
      "md:flex",
    );
    expect(screen.getByTestId("patchbay-icon")).toBeInTheDocument();
  });
});
