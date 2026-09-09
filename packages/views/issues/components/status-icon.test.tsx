// @vitest-environment jsdom

import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { PriorityIcon } from "./priority-icon";
import { StatusIcon } from "./status-icon";

describe("issue icons", () => {
  it("renders a muted fallback for unknown status values", () => {
    const { container } = render(<StatusIcon status="unexpected_status" />);

    const icon = container.querySelector("svg");
    expect(icon).toHaveClass("text-muted-foreground");
  });

  it.each([
    ["backlog", "lucide-circle-dot"],
    ["todo", "lucide-circle"],
    ["in_progress", "lucide-circle-dot"],
    ["in_review", "lucide-clock"],
    ["done", "lucide-circle-check"],
    ["blocked", "lucide-circle-x"],
    ["cancelled", "lucide-circle-pause"],
  ])("uses the ReUI icon family for %s", (status, className) => {
    const { container } = render(<StatusIcon status={status} />);
    expect(container.querySelector("svg")).toHaveClass(className);
  });

  it("retains custom category and color semantics", () => {
    const { container, rerender } = render(<StatusIcon status="qa" category="in_review" color="#ec7a2d" />);
    expect(container.querySelector("svg")).toHaveClass("lucide-clock");
    expect(container.querySelector("svg")).toHaveStyle({ color: "#ec7a2d" });
    rerender(<StatusIcon status="qa" category="in_review" color="#ec7a2d" inheritColor />);
    expect(container.querySelector("svg")?.style.color).toBe("");
  });

  it("renders a muted fallback for unknown priority values", () => {
    const { container } = render(<PriorityIcon priority="unexpected_priority" />);

    const icon = container.querySelector("svg");
    expect(icon).toHaveClass("text-muted-foreground");
  });
});
