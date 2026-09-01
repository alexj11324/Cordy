// @vitest-environment jsdom

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { I18nProvider } from "@patchbay/core/i18n/react";
import enChat from "../../locales/en/chat.json";
import enIssues from "../../locales/en/issues.json";
import { SessionModePicker } from "./session-mode-picker";

const TEST_RESOURCES = { en: { chat: enChat, issues: enIssues } };

function renderPicker(
  props: Partial<React.ComponentProps<typeof SessionModePicker>> = {},
) {
  const onChange = vi.fn();
  const utils = render(
    <I18nProvider locale="en" resources={TEST_RESOURCES}>
      <SessionModePicker
        value=""
        advertised={[
          { value: "auto", label: "Approve for me", kind: "auto_review" },
          { value: "ask", label: "Ask" },
        ]}
        canEdit
        onChange={onChange}
        {...props}
      />
    </I18nProvider>,
  );
  return { ...utils, onChange };
}

describe("SessionModePicker", () => {
  beforeEach(() => {
    cleanup();
  });
  afterEach(() => {
    cleanup();
  });

  it("labels empty persistence as full access", () => {
    renderPicker({ value: "" });
    expect(screen.getAllByText("Full access").length).toBeGreaterThan(0);
  });

  it("shows the advertised auto label, not a provider-hardcoded name", () => {
    renderPicker({ value: "auto" });
    expect(screen.getAllByText("Approve for me").length).toBeGreaterThan(0);
  });

  it("always offers full access and advertised auto, never ask", () => {
    const { onChange } = renderPicker({ value: "" });
    fireEvent.click(screen.getByRole("button"));
    expect(screen.getByText("Approve for me")).toBeInTheDocument();
    expect(screen.queryByText("Ask")).not.toBeInTheDocument();
    fireEvent.click(screen.getByText("Approve for me"));
    expect(onChange).toHaveBeenCalledWith("auto");
  });

  it("clears back to full access", () => {
    const { onChange } = renderPicker({ value: "auto" });
    fireEvent.click(screen.getByRole("button"));
    const fullAccess = screen
      .getAllByRole("button")
      .find(
        (button) =>
          button.getAttribute("data-picker-item") !== null &&
          button.textContent?.includes("Full access"),
      );
    expect(fullAccess).toBeDefined();
    fireEvent.click(fullAccess!);
    expect(onChange).toHaveBeenCalledWith("");
  });

  it("renders a static label when the agent cannot be edited", () => {
    renderPicker({ value: "", canEdit: false });
    expect(screen.getByText("Full access")).toBeInTheDocument();
    expect(screen.queryByRole("button")).toBeNull();
  });
});
