import { screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { ComponentProps } from "react";
import type {
  AutomationToolsConfig,
} from "@patchbay/core/automations";
import type {
  SlackAutomationChannel,
  SlackInstallation,
} from "@patchbay/core/types";
import { renderWithI18n } from "../../test/i18n";
import { AutomationSlackToolRow } from "./automation-slack-tool-row";

vi.mock("../../navigation", () => ({
  AppLink: ({ href, ...props }: ComponentProps<"a">) => <a href={href} {...props} />,
}));

const installations = [
  { id: "install-1", team_id: "T1", status: "installed" },
  { id: "install-2", team_id: "T2", status: "installed" },
] as SlackInstallation[];

const channels: SlackAutomationChannel[] = [
  { installation_id: "install-1", team_id: "T1", id: "C1", name: "general" },
  { installation_id: "install-1", team_id: "T1", id: "C2", name: "incidents" },
  { installation_id: "install-2", team_id: "T2", id: "C3", name: "launch" },
];

type PersistMock = ReturnType<typeof vi.fn<(next: AutomationToolsConfig) => void>>;

function renderRow({
  tools = {},
  onPersist = vi.fn<(next: AutomationToolsConfig) => void>(),
  rows = installations,
  catalog = channels,
}: {
  tools?: AutomationToolsConfig;
  onPersist?: PersistMock;
  rows?: SlackInstallation[];
  catalog?: SlackAutomationChannel[];
} = {}) {
  const result = renderWithI18n(
    <AutomationSlackToolRow
      tools={tools}
      installations={rows}
      channels={catalog}
      canWrite
      busy={false}
      catalogPending={false}
      catalogError={false}
      connectHref="/acme/settings?tab=integrations"
      onRetry={vi.fn()}
      onPersist={onPersist}
      onRemove={vi.fn()}
    />,
  );
  return {
    ...result,
    onPersist,
    rerenderBusy: (busy: boolean) => result.rerender(
      <AutomationSlackToolRow
        tools={tools}
        installations={rows}
        channels={catalog}
        canWrite
        busy={busy}
        catalogPending={false}
        catalogError={false}
        connectHref="/acme/settings?tab=integrations"
        onRetry={vi.fn()}
        onPersist={onPersist}
        onRemove={vi.fn()}
      />,
    ),
  };
}

describe("AutomationSlackToolRow", () => {
  it("shows the real workspace and selected catalog channels for an enabled target", async () => {
    const user = userEvent.setup();
    renderRow({
      tools: {
        slack_send: {
          enabled: true,
          installation_id: "install-1",
          channel_ids: ["C1", "C1", "C2"],
        },
      },
    });

    expect(screen.queryByRole("switch", { name: "Send to Slack" })).not.toBeInTheDocument();
    expect(screen.getByRole("combobox", { name: "Slack workspace" })).toHaveTextContent("T1");
    const channelPicker = screen.getByRole("button", { name: "Slack channels" });
    expect(channelPicker).toHaveTextContent("2 channels");
    await user.click(channelPicker);
    expect(screen.getByRole("checkbox", { name: "#general" })).toBeChecked();
    expect(screen.getByRole("checkbox", { name: "#incidents" })).toBeChecked();
    expect(screen.queryByRole("checkbox", { name: "#launch" })).not.toBeInTheDocument();
  });

  it("persists a target after a real catalog channel has been selected", async () => {
    const user = userEvent.setup();
    const { onPersist } = renderRow({ rows: [installations[0]!], tools: {} });
    expect(screen.queryByRole("textbox", { name: /channel/i })).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Slack channels" }));
    await user.click(screen.getByRole("checkbox", { name: "#general" }));
    expect(onPersist).toHaveBeenLastCalledWith({
      slack_send: {
        installation_id: "install-1",
        channel_ids: ["C1"],
      },
    });
  });

  it("filters channels by the selected installed Slack workspace", async () => {
    const user = userEvent.setup();
    renderRow();

    expect(screen.getByRole("button", { name: "Slack channels" })).toBeDisabled();
    await user.click(screen.getByRole("combobox", { name: "Slack workspace" }));
    await user.click(await screen.findByRole("option", { name: "Slack workspace T2" }));
    await user.click(screen.getByRole("button", { name: "Slack channels" }));
    expect(screen.getByRole("checkbox", { name: "#launch" })).toBeInTheDocument();
    expect(screen.queryByRole("checkbox", { name: "#general" })).not.toBeInTheDocument();
  });

  it("reads a uniquely matching legacy channel without fabricating a target", async () => {
    const user = userEvent.setup();
    const { onPersist } = renderRow({
      rows: [installations[0]!],
      tools: { slack_send: { enabled: false, channel: "#general" } },
    });

    const channelPicker = screen.getByRole("button", { name: "Slack channels" });
    expect(channelPicker).toHaveTextContent("#general");
    await user.click(channelPicker);
    expect(screen.getByRole("checkbox", { name: "#general" })).toBeChecked();
    expect(onPersist).not.toHaveBeenCalled();
  });

  it("does not fabricate a target when the catalog has no channels", () => {
    renderRow({ rows: [installations[0]!], catalog: [] });
    expect(screen.getByRole("button", { name: "Slack channels" })).toHaveTextContent("Select channels");
    expect(screen.queryByRole("textbox", { name: /channel/i })).not.toBeInTheDocument();
  });

  it("keeps an invalid existing installation visible without fabricating its missing channel", async () => {
    renderRow({
      tools: {
        slack_send: {
          enabled: true,
          installation_id: "install-2",
          channel_ids: ["C-deleted"],
        },
      },
    });

    expect(screen.getByRole("combobox", { name: "Slack workspace" })).toHaveTextContent("T2");
    expect(screen.getByRole("button", { name: "Slack channels" })).toHaveTextContent("Select channels");
    await userEvent.setup().click(screen.getByRole("button", { name: "Slack channels" }));
    expect(screen.getByRole("checkbox", { name: "#launch" })).not.toBeChecked();
  });

  it("trusts the canonical revoked lifecycle state over the legacy installed alias", () => {
    renderRow({
      rows: [{
        ...installations[0]!,
        status: "installed",
        installation_status: "revoked",
      }],
    });

    expect(screen.getByRole("button", { name: /Connect/ })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Slack channels" })).not.toBeInTheDocument();
  });

  it("locks channel choices when a save starts with the popover already open", async () => {
    const user = userEvent.setup();
    const { onPersist, rerenderBusy } = renderRow({
      rows: [installations[0]!],
      tools: {
        slack_send: {
          enabled: false,
          installation_id: "install-1",
          channel_ids: ["C1"],
        },
      },
    });
    await user.click(screen.getByRole("button", { name: "Slack channels" }));
    const channel = screen.getByRole("checkbox", { name: "#general" });

    rerenderBusy(true);
    expect(channel).toHaveAttribute("aria-disabled", "true");
    await user.click(channel);
    expect(onPersist).not.toHaveBeenCalled();
  });
});
