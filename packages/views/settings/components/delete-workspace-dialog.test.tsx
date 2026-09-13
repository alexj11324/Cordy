// @vitest-environment jsdom

import { describe, expect, it, beforeEach, vi } from "vitest";
import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { renderWithI18n } from "../../test/i18n";
import { DeleteWorkspaceDialog } from "./delete-workspace-dialog";

// The shadcn `Dialog` module is no longer mocked. It used to be stripped to
// pass-through wrappers because it was "a Base UI portal that's awkward to
// test" — but the dialog is Lobe's base-ui `Modal` now, and the typed-
// confirmation logic it was isolating is still in the body. Mocking a module
// the component no longer imports would have left the suite testing a stub.
//
// `{ lobe: true }` is required, and async: the bridge's module resolves lazily,
// so until it does the tree is a `Suspense` fallback of `null`.
async function renderDialog(props: {
  workspaceName: string;
  open: boolean;
  loading?: boolean;
  onOpenChange?: (open: boolean) => void;
  onConfirm?: () => void;
}) {
  const result = renderWithI18n(
    <DeleteWorkspaceDialog
      loading={props.loading}
      open={props.open}
      onConfirm={props.onConfirm ?? vi.fn()}
      onOpenChange={props.onOpenChange ?? vi.fn()}
      workspaceName={props.workspaceName}
    />,
    { lobe: true },
  );
  await screen.findByRole("textbox");
  return result;
}

describe("DeleteWorkspaceDialog", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("disables Delete when input is empty", async () => {
    await renderDialog({
      workspaceName: "acme",
      open: true,
    });
    expect(screen.getByRole("button", { name: "Delete workspace" })).toBeDisabled();
  });

  it("keeps Delete disabled when input doesn't match (case-sensitive)", async () => {
    const user = userEvent.setup();
    await renderDialog({
      workspaceName: "acme",
      open: true,
    });

    await user.type(screen.getByRole("textbox"), "ACME"); // wrong case
    expect(screen.getByRole("button", { name: "Delete workspace" })).toBeDisabled();

    await user.clear(screen.getByRole("textbox"));
    await user.type(screen.getByRole("textbox"), "acme "); // trailing space
    expect(screen.getByRole("button", { name: "Delete workspace" })).toBeDisabled();
  });

  it("enables Delete on exact match and calls onConfirm when clicked", async () => {
    const user = userEvent.setup();
    const onConfirm = vi.fn();
    await renderDialog({
      workspaceName: "acme",
      open: true,
      onConfirm,
    });

    await user.type(screen.getByRole("textbox"), "acme");
    const deleteBtn = screen.getByRole("button", { name: "Delete workspace" });
    expect(deleteBtn).toBeEnabled();

    await user.click(deleteBtn);
    expect(onConfirm).toHaveBeenCalledTimes(1);
  });

  it("submits on Enter when matched; ignores Enter when not matched", async () => {
    const user = userEvent.setup();
    const onConfirm = vi.fn();
    await renderDialog({
      workspaceName: "acme",
      open: true,
      onConfirm,
    });

    const input = screen.getByRole("textbox");
    await user.type(input, "acm{Enter}"); // not yet matched
    expect(onConfirm).not.toHaveBeenCalled();

    await user.type(input, "e{Enter}"); // now matches "acme"
    expect(onConfirm).toHaveBeenCalledTimes(1);
  });

  it("Cancel closes the dialog and does not call onConfirm", async () => {
    const user = userEvent.setup();
    const onOpenChange = vi.fn();
    const onConfirm = vi.fn();
    await renderDialog({
      workspaceName: "acme",
      open: true,
      onOpenChange,
      onConfirm,
    });

    await user.click(screen.getByRole("button", { name: "Cancel" }));
    expect(onOpenChange).toHaveBeenCalledWith(false);
    expect(onConfirm).not.toHaveBeenCalled();
  });

  it("shows loading state and disables both buttons while pending", async () => {
    await renderDialog({
      workspaceName: "acme",
      loading: true,
      open: true,
    });
    expect(screen.getByRole("button", { name: "Deleting..." })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Cancel" })).toBeDisabled();
  });

  it("matches names with spaces, unicode, and other non-ASCII characters literally", async () => {
    const user = userEvent.setup();
    const onConfirm = vi.fn();
    await renderDialog({
      workspaceName: "My 团队 🚀",
      open: true,
      onConfirm,
    });
    const input = screen.getByRole("textbox");
    await user.type(input, "My 团队 🚀");
    expect(screen.getByRole("button", { name: "Delete workspace" })).toBeEnabled();
    await user.click(screen.getByRole("button", { name: "Delete workspace" }));
    expect(onConfirm).toHaveBeenCalledTimes(1);
  });

  it("resets the input when the workspace being deleted changes (e.g. rename mid-dialog)", async () => {
    const user = userEvent.setup();
    const { rerender } = await renderDialog({
      workspaceName: "old-name",
      open: true,
    });
    // Typed, not assigned: writing `input.value` directly skips React's state,
    // so the reset below would have held even if the effect did nothing. Going
    // through `userEvent` is what makes the re-render able to go wrong.
    await user.type(screen.getByRole("textbox"), "old-name");
    expect(screen.getByRole("button", { name: "Delete workspace" })).toBeEnabled();

    rerender(
      <DeleteWorkspaceDialog
        workspaceName="new-name"
        open
        onOpenChange={vi.fn()}
        onConfirm={vi.fn()}
      />,
    );
    expect(screen.getByRole("textbox")).toHaveValue("");
    // And the confirmation is re-armed for the new name rather than left armed
    // by the old one: the button that was enabled a line ago is disabled again.
    expect(screen.getByRole("button", { name: "Delete workspace" })).toBeDisabled();
  });

  it("clears the input when reopened so prior attempts don't leak", async () => {
    const user = userEvent.setup();
    const { rerender } = await renderDialog({
      workspaceName: "acme",
      open: true,
    });

    await user.type(screen.getByRole("textbox"), "partial");
    expect(screen.getByRole("textbox")).toHaveValue("partial");

    // Simulate close → reopen (e.g. user canceled, then clicked Delete again).
    // The close is waited on: the popup leaves through an exit animation, so a
    // synchronous reopen would leave two inputs in the document.
    rerender(
      <DeleteWorkspaceDialog
        workspaceName="acme"
        open={false}
        onOpenChange={vi.fn()}
        onConfirm={vi.fn()}
      />,
    );
    await waitFor(() => expect(screen.queryByRole("textbox")).toBeNull());

    rerender(
      <DeleteWorkspaceDialog
        workspaceName="acme"
        open
        onOpenChange={vi.fn()}
        onConfirm={vi.fn()}
      />,
    );
    expect(await screen.findByRole("textbox")).toHaveValue("");
  });
});
