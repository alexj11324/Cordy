import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { ClerkAuthShell } from "./clerk-auth-shell";

vi.mock("@patchbay/ui/components/common/patchbay-icon", () => ({
  PatchbayIcon: () => <div data-testid="patchbay-icon" />,
}));

describe("ClerkAuthShell", () => {
  it("renders the approved white-left, black-right shadcn layout", () => {
    render(
      <ClerkAuthShell>
        <div data-testid="auth-form" />
      </ClerkAuthShell>,
    );

    expect(screen.getByTestId("clerk-auth-shell")).toHaveClass(
      "bg-white",
      "md:grid-cols-2",
    );
    expect(screen.getByTestId("auth-form").parentElement).toHaveClass(
      "bg-white",
    );
    expect(screen.getByTestId("clerk-auth-brand-panel")).toHaveClass(
      "bg-zinc-950",
      "md:flex",
    );
    expect(screen.getByTestId("patchbay-icon")).toBeInTheDocument();
  });
});
