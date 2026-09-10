// @vitest-environment jsdom

import React from "react";
import { fireEvent, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { renderWithI18n } from "../../test/i18n";
import type { ProjectBuilderDraft } from "./project-builder-draft";

const h = vi.hoisted(() => ({
  setDraft: vi.fn(),
  draft: {
    title: "Existing title",
    summary: "Existing summary",
    description: "Existing description",
    status: "planned" as const,
    priority: "medium" as const,
    leadType: "member" as const,
    leadId: "member-1",
    icon: "📁",
    startDate: "2026-09-10",
    dueDate: "2026-09-30",
  },
}));

vi.mock("@orvilo/core/projects", () => ({
  useProjectDraftStore: (selector: (state: typeof h) => unknown) => selector(h),
}));

vi.mock("@orvilo/ui/components/ui/dialog", () => ({
  Dialog: ({ children }: { children: React.ReactNode }) => <div>{children}</div>,
  DialogContent: ({ children }: { children: React.ReactNode }) => <div>{children}</div>,
  DialogDescription: ({ children }: { children: React.ReactNode }) => <p>{children}</p>,
  DialogHeader: ({ children }: { children: React.ReactNode }) => <div>{children}</div>,
  DialogTitle: ({ children }: { children: React.ReactNode }) => <h1>{children}</h1>,
}));

vi.mock("@orvilo/ui/components/ui/button", () => ({
  Button: ({
    children,
    onClick,
  }: {
    children: React.ReactNode;
    onClick?: () => void;
  }) => (
    <button type="button" onClick={onClick}>
      {children}
    </button>
  ),
}));

vi.mock("../create-project", () => ({
  CreateProjectModal: ({ onClose }: { onClose: () => void }) => (
    <div>
      manual project form
      <button type="button" onClick={onClose}>close manual</button>
    </div>
  ),
}));

vi.mock("./project-builder-panel", () => ({
  ProjectBuilderPanel: ({
    currentDraft,
    onApply,
    onClose,
    onSwitchToManual,
  }: {
    currentDraft: ProjectBuilderDraft;
    onApply: (draft: ProjectBuilderDraft) => void;
    onClose: () => void;
    onSwitchToManual: () => void;
  }) => (
    <div>
      <output data-testid="builder-draft">{JSON.stringify(currentDraft)}</output>
      <button
        type="button"
        onClick={() =>
          onApply({
            title: "Assistant title",
            summary: "Assistant summary",
            description: "Assistant description",
            icon: "🚀",
            status: "in_progress",
            priority: "urgent",
            lead_type: "agent",
            lead_id: "agent-1",
            start_date: "2026-10-01",
            due_date: null,
            member_ids: ["member-1"],
            label_ids: ["label-1"],
            dependency_ids: ["project-1"],
          })
        }
      >
        apply proposal
      </button>
      <button type="button" onClick={onSwitchToManual}>manual from builder</button>
      <button type="button" onClick={onClose}>close builder</button>
    </div>
  ),
}));

import { CreateProjectFlowModal } from "./create-project-flow";

describe("CreateProjectFlowModal", () => {
  beforeEach(() => {
    h.setDraft.mockClear();
  });

  it("keeps manual create available from the registered project flow", () => {
    const onClose = vi.fn();
    renderWithI18n(<CreateProjectFlowModal onClose={onClose} />);

    fireEvent.click(screen.getByRole("button", { name: "Create manually" }));
    expect(screen.getByText("manual project form")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "close manual" }));
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("opens the builder with the current form draft and applies its proposal", () => {
    renderWithI18n(<CreateProjectFlowModal onClose={vi.fn()} />);

    fireEvent.click(screen.getByRole("button", { name: "Create with Agent" }));
    expect(JSON.parse(screen.getByTestId("builder-draft").textContent ?? "")).toEqual({
      title: "Existing title",
      summary: "Existing summary",
      description: "Existing description",
      icon: "📁",
      status: "planned",
      priority: "medium",
      lead_type: "member",
      lead_id: "member-1",
      start_date: "2026-09-10",
      due_date: "2026-09-30",
      member_ids: [],
      label_ids: [],
      dependency_ids: [],
    });

    fireEvent.click(screen.getByRole("button", { name: "apply proposal" }));
    expect(h.setDraft).toHaveBeenCalledWith({
      title: "Assistant title",
      summary: "Assistant summary",
      description: "Assistant description",
      icon: "🚀",
      status: "in_progress",
      priority: "urgent",
      leadType: "agent",
      leadId: "agent-1",
      startDate: "2026-10-01",
      dueDate: undefined,
    });
    expect(screen.getByText("manual project form")).toBeInTheDocument();
  });

  it("can leave the builder for the unchanged manual form", () => {
    renderWithI18n(<CreateProjectFlowModal onClose={vi.fn()} />);

    fireEvent.click(screen.getByRole("button", { name: "Create with Agent" }));
    fireEvent.click(screen.getByRole("button", { name: "manual from builder" }));

    expect(h.setDraft).not.toHaveBeenCalled();
    expect(screen.getByText("manual project form")).toBeInTheDocument();
  });
});
