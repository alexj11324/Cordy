// @vitest-environment jsdom

import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { PropRow } from "./prop-row";

describe("PropRow", () => {
  it("keeps hover chrome on the rounded label/value content instead of the empty row", () => {
    const { container } = render(<PropRow label="Executor">Codex</PropRow>);
    const row = container.firstElementChild;
    const label = screen.getByText("Executor");
    const value = screen.getByText("Codex");
    const valueTrack = value.parentElement;

    expect(row).not.toHaveClass("hover:bg-accent/50");
    expect(label).toHaveClass("rounded-l-md");
    expect(value).toHaveClass("max-w-full", "rounded-r-md");
    expect(valueTrack).toHaveClass("min-w-0");
    expect(valueTrack).not.toHaveClass("w-fit");
  });
});
