import { screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { renderWithI18n } from "../../test/i18n";
import { AutomationTemplateGallery } from "./automation-template-gallery";
import { AUTOMATION_TEMPLATES } from "./automation-templates";

describe("AutomationTemplateGallery", () => {
  it("renders text-only popular cards without brand imagery", () => {
    renderWithI18n(
      <AutomationTemplateGallery
        onSelectTemplate={vi.fn()}
        onStartBlank={vi.fn()}
      />,
    );

    expect(
      screen.getByRole("tab", { name: "Popular" }),
    ).toHaveAttribute("aria-selected", "true");
    expect(screen.getByText("Find critical bugs")).toBeInTheDocument();
    expect(
      screen.getByText(
        "Analyze recent commits for high-severity correctness bugs and submit safe fixes",
      ),
    ).toBeInTheDocument();
    expect(screen.queryByText(/Scheduled →/)).not.toBeInTheDocument();
    expect(screen.getByRole("tabpanel").querySelector("img")).toBeNull();
  });

  it("switches category cards from the tabs", async () => {
    const user = userEvent.setup();
    renderWithI18n(
      <AutomationTemplateGallery
        onSelectTemplate={vi.fn()}
        onStartBlank={vi.fn()}
      />,
    );

    await user.click(screen.getByRole("tab", { name: "Environment" }));

    const panel = screen.getByRole("tabpanel");
    expect(panel).toHaveTextContent("Investigate environment setup failures");
    expect(panel).toHaveTextContent("Monitor environment build health");
    expect(panel).not.toHaveTextContent("Find critical bugs");
  });

  it("opens the selected template and the blank path", async () => {
    const user = userEvent.setup();
    const onSelectTemplate = vi.fn();
    const onStartBlank = vi.fn();
    renderWithI18n(
      <AutomationTemplateGallery
        onSelectTemplate={onSelectTemplate}
        onStartBlank={onStartBlank}
      />,
    );

    await user.click(
      screen.getByRole("button", { name: /Find critical bugs/ }),
    );
    expect(onSelectTemplate).toHaveBeenCalledWith(
      AUTOMATION_TEMPLATES.find_critical_bugs,
    );

    await user.click(
      screen.getByRole("button", { name: "Start from scratch" }),
    );
    expect(onStartBlank).toHaveBeenCalledTimes(1);
  });
});
