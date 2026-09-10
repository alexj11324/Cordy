// @vitest-environment jsdom

import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

const h = vi.hoisted(() => ({
  close: vi.fn(),
  state: {
    modal: "create-project" as string | null,
    data: null as Record<string, unknown> | null,
  },
}));

vi.mock("@orvilo/core/modals", () => ({
  useModalStore: (selector: (state: typeof h.state & { close: typeof h.close }) => unknown) =>
    selector({ ...h.state, close: h.close }),
}));

vi.mock("./project-builder/create-project-flow", () => ({
  CreateProjectFlowModal: () => <div>reachable project builder flow</div>,
}));
vi.mock("./create-issue-dialog", () => ({ CreateIssueDialog: () => null }));
vi.mock("./create-team", () => ({ CreateTeamModal: () => null }));
vi.mock("./feedback", () => ({ FeedbackModal: () => null }));
vi.mock("./set-parent-issue", () => ({ SetParentIssueModal: () => null }));
vi.mock("./add-child-issue", () => ({ AddChildIssueModal: () => null }));
vi.mock("./delete-issue-confirm", () => ({ DeleteIssueConfirmModal: () => null }));
vi.mock("./run-confirm", () => ({ RunConfirmModal: () => null }));
vi.mock("./issue-limit-upgrade-dialog", () => ({ IssueLimitUpgradeDialog: () => null }));

import { ModalRegistry } from "./registry";

describe("ModalRegistry", () => {
  it("routes create-project through the flow that exposes the project builder", () => {
    render(<ModalRegistry />);

    expect(screen.getByText("reachable project builder flow")).toBeInTheDocument();
  });
});
