// @vitest-environment jsdom

import { describe, expect, it, vi, afterEach } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { ConnectionSelectField, CredentialFieldForm } from "./credential-field-form";

afterEach(cleanup);

function renderForm(onSubmit = () => undefined, onCancel = () => undefined) {
  function Harness() {
    return (
      <CredentialFieldForm
        fields={[
          {
            id: "app-key",
            label: "AppKey",
            value: "ding-key",
            onChange: () => undefined,
            testId: "app-key",
          },
        ]}
        cancelLabel="Cancel"
        submitLabel="Connect"
        canSubmit
        onCancel={onCancel}
        onSubmit={onSubmit}
        submitTestId="submit"
      />
    );
  }
  return render(<Harness />);
}

describe("CredentialFieldForm", () => {
  it("uses the c-field-9 outline cancel + default submit row", async () => {
    const onSubmit = vi.fn();
    const onCancel = vi.fn();
    renderForm(onSubmit, onCancel);
    expect(screen.getByLabelText("AppKey")).toBeTruthy();
    await userEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(onCancel).toHaveBeenCalled();
    await userEvent.click(screen.getByTestId("submit"));
    expect(onSubmit).toHaveBeenCalled();
  });

  it("keeps a placeholder item so a connection selection can be cleared", async () => {
    const onValueChange = vi.fn();
    render(
      <ConnectionSelectField
        id="linear-status"
        label="Todo"
        value="backlog"
        placeholder="Leave unmapped"
        items={[{ value: "backlog", label: "Backlog" }]}
        onValueChange={onValueChange}
      />,
    );

    await userEvent.click(screen.getByRole("combobox", { name: "Todo" }));
    await userEvent.click(await screen.findByRole("option", { name: "Leave unmapped" }));
    expect(onValueChange).toHaveBeenCalledWith("");
  });
});
