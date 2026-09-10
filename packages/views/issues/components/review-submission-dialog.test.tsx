import { act, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { renderWithI18n } from "../../test/i18n";
import { ReviewSubmissionDialog } from "./review-submission-dialog";

vi.mock("./pickers/executor-picker", () => ({
  ReviewerPicker: ({
    reviewerType,
    reviewerId,
    onUpdate,
  }: {
    reviewerType: string | null;
    reviewerId: string | null;
    onUpdate: (value: { reviewer_type: "member"; reviewer_id: string }) => void;
  }) => (
    <button
      type="button"
      data-testid="reviewer-picker"
      data-reviewer={`${reviewerType ?? ""}:${reviewerId ?? ""}`}
      onClick={() =>
        onUpdate({ reviewer_type: "member", reviewer_id: "reviewer-1" })
      }
    >
      Choose reviewer
    </button>
  ),
}));

async function completeForm() {
  const user = userEvent.setup();
  await user.type(screen.getByLabelText("Worktree path"), "/work/project");
  await user.type(screen.getByLabelText("Branch"), "codex/review");
  await user.type(
    screen.getByLabelText("Full commit SHA"),
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
  );
  await user.type(
    screen.getByLabelText(/PR URLs/),
    "https://github.com/example/project/pull/42",
  );
  await user.click(screen.getByRole("button", { name: "Choose reviewer" }));
  return user;
}

describe("ReviewSubmissionDialog", () => {
  it("waits for the write and preserves every field when it fails", async () => {
    let fail!: () => void;
    const onSubmit = vi.fn(
      () =>
        new Promise<boolean>((_resolve, reject) => {
          fail = () => reject(new Error("server rejected review"));
        }),
    );
    const onClose = vi.fn();
    renderWithI18n(
      <ReviewSubmissionDialog
        status="in_review"
        onClose={onClose}
        onSubmit={onSubmit}
      />,
    );
    const user = await completeForm();

    await user.click(screen.getByRole("button", { name: "Submit for review" }));

    expect(onSubmit).toHaveBeenCalledTimes(1);
    expect(onClose).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "Submit for review" })).toBeDisabled();

    await act(async () => fail());

    await waitFor(() =>
      expect(screen.getByRole("button", { name: "Submit for review" })).toBeEnabled(),
    );
    expect(onClose).not.toHaveBeenCalled();
    expect(screen.getByLabelText("Worktree path")).toHaveValue("/work/project");
    expect(screen.getByLabelText("Branch")).toHaveValue("codex/review");
    expect(screen.getByLabelText("Full commit SHA")).toHaveValue(
      "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    );
    expect(screen.getByLabelText(/PR URLs/)).toHaveValue(
      "https://github.com/example/project/pull/42",
    );
    expect(screen.getByTestId("reviewer-picker")).toHaveAttribute(
      "data-reviewer",
      "member:reviewer-1",
    );
  });

  it("closes only after the write succeeds", async () => {
    let finish!: (accepted: boolean) => void;
    const onSubmit = vi.fn(
      () => new Promise<boolean>((resolve) => { finish = resolve; }),
    );
    const onClose = vi.fn();
    renderWithI18n(
      <ReviewSubmissionDialog
        status="in_review"
        onClose={onClose}
        onSubmit={onSubmit}
      />,
    );
    const user = await completeForm();

    await user.click(screen.getByRole("button", { name: "Submit for review" }));
    expect(onClose).not.toHaveBeenCalled();

    await act(async () => finish(true));
    await waitFor(() => expect(onClose).toHaveBeenCalledTimes(1));
  });
});
