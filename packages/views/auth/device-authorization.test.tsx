// @vitest-environment jsdom
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

const { inspectDeviceAuthorization, decideDeviceAuthorization } = vi.hoisted(
  () => ({
    inspectDeviceAuthorization: vi.fn(),
    decideDeviceAuthorization: vi.fn(),
  }),
);

vi.mock("@orvilo/core/api", () => ({
  api: { inspectDeviceAuthorization, decideDeviceAuthorization },
}));

vi.mock("../i18n", () => ({
  useT: () => ({
    t: (
      selector: (resources: { device: Record<string, string> }) => string,
      values?: Record<string, string>,
    ) => {
      const value = selector({
        device: {
          title: "Authorize a client",
          code_label: "One-time code",
          code_hint: "Enter the code shown by orvilo login.",
          continue: "Continue",
          confirm: "Allow {{name}} to access your account?",
          scope: "Approve only a login you started.",
          approve: "Authorize",
          deny: "Deny",
          approved: "Authorized. Return to your terminal.",
          denied: "Authorization denied.",
          invalid_code: "Unable to verify this code.",
          decision_failed: "Unable to complete authorization.",
        },
      });
      return values?.name ? value.replace("{{name}}", values.name) : value;
    },
  }),
}));

import { DeviceAuthorization } from "./device-authorization";

describe("DeviceAuthorization", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    inspectDeviceAuthorization.mockResolvedValue({
      client_name: "CLI (oracle)",
      expires_at: "2026-09-10T05:30:00Z",
    });
    decideDeviceAuthorization.mockResolvedValue(undefined);
  });

  it("confirms the requesting client before approval", async () => {
    const user = userEvent.setup();
    render(<DeviceAuthorization />);

    await user.type(screen.getByLabelText("One-time code"), "abcd-efgh");
    await user.click(screen.getByRole("button", { name: "Continue" }));

    expect(inspectDeviceAuthorization).toHaveBeenCalledWith("ABCD-EFGH");
    expect(
      await screen.findByText("Allow CLI (oracle) to access your account?"),
    ).toBeInTheDocument();
    expect(screen.getByText("ABCD-EFGH")).toBeInTheDocument();
  });

  it("sends approval and renders the terminal handoff", async () => {
    const user = userEvent.setup();
    render(<DeviceAuthorization />);

    await user.type(screen.getByLabelText("One-time code"), "ABCD-EFGH");
    await user.click(screen.getByRole("button", { name: "Continue" }));
    await user.click(await screen.findByRole("button", { name: "Authorize" }));

    expect(decideDeviceAuthorization).toHaveBeenCalledWith("ABCD-EFGH", true);
    expect(await screen.findByRole("status")).toHaveTextContent(
      "Authorized. Return to your terminal.",
    );
  });

  it("keeps an invalid code on the entry screen", async () => {
    inspectDeviceAuthorization.mockRejectedValueOnce(new Error("invalid"));
    const user = userEvent.setup();
    render(<DeviceAuthorization />);

    await user.type(screen.getByLabelText("One-time code"), "NOPE-NOPE");
    await user.click(screen.getByRole("button", { name: "Continue" }));

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Unable to verify this code.",
    );
    expect(screen.getByLabelText("One-time code")).toBeInTheDocument();
  });
});
