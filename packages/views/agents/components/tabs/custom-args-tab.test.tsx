// @vitest-environment jsdom

import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { toast } from "sonner";
import type { Agent } from "@orvilo/core/types";
import { I18nProvider } from "@orvilo/core/i18n/react";
import enCommon from "../../../locales/en/common.json";
import enAgents from "../../../locales/en/agents.json";

const TEST_RESOURCES = { en: { common: enCommon, agents: enAgents } };

vi.mock("sonner", () => ({
  toast: {
    error: vi.fn(),
    success: vi.fn(),
  },
}));

import { CustomArgsTab } from "./custom-args-tab";

const baseAgent: Agent = {
  id: "agent-1",
  workspace_id: "ws-1",
  runtime_id: "runtime-1",
  name: "Agent",
  description: "",
  instructions: "",
  avatar_url: null,
  runtime_mode: "local",
  runtime_config: {},
  custom_args: ["--profile", "research"],
  visibility: "workspace",
  permission_mode: "public_to",
  invocation_targets: [{ target_type: "workspace", target_id: null }],
  status: "idle",
  max_concurrent_tasks: 1,
  model: "",
  owner_id: "user-1",
  skills: [],
  created_at: "2026-05-28T00:00:00Z",
  updated_at: "2026-05-28T00:00:00Z",
  archived_at: null,
  archived_by: null,
};

function renderTab(
  overrides: Partial<Agent> = {},
  onSave = vi.fn().mockResolvedValue(undefined),
  compact = false,
  onDirtyChange = vi.fn(),
) {
  const result = render(
    <I18nProvider locale="en" resources={TEST_RESOURCES}>
      <CustomArgsTab
        compact={compact}
        agent={{ ...baseAgent, ...overrides }}
        onSave={onSave}
        onDirtyChange={onDirtyChange}
      />
    </I18nProvider>,
  );

  return { ...result, onSave, onDirtyChange };
}

describe("CustomArgsTab", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("renders configured arguments as a list, not persistent inputs", () => {
    renderTab();

    expect(screen.getByText("--profile")).toBeInTheDocument();
    expect(screen.getByText("research")).toBeInTheDocument();
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /^save$/i })).not.toBeInTheDocument();
    expect(screen.queryByText("codex app-server --profile research")).not.toBeInTheDocument();
  });

  it("uses one editor to add one argument token", async () => {
    const user = userEvent.setup();
    const { onSave, onDirtyChange } = renderTab({ custom_args: [] });

    await user.click(screen.getByRole("button", { name: /add argument/i }));
    const input = screen.getByRole("textbox", { name: /new argument/i });
    await user.type(input, "value with spaces");
    await user.click(screen.getByRole("button", { name: /^add$/i }));

    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
    expect(screen.getByText("value with spaces")).toBeInTheDocument();
    expect(onSave).toHaveBeenCalledWith({ custom_args: ["value with spaces"] });
    expect(onDirtyChange).toHaveBeenLastCalledWith(false);
  });

  it("edits a list item in place with the same single editor", async () => {
    const user = userEvent.setup();
    const { onSave } = renderTab();

    await user.click(screen.getByRole("button", { name: /edit argument 1/i }));
    const input = screen.getByRole("textbox", { name: /argument 1/i });
    await user.clear(input);
    await user.type(input, "--model");
    await user.click(screen.getByRole("button", { name: /update/i }));

    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
    expect(screen.getByText("--model")).toBeInTheDocument();
    expect(screen.queryByText("--profile")).not.toBeInTheDocument();
    expect(onSave).toHaveBeenCalledWith({ custom_args: ["--model", "research"] });
  });

  it.each([false, true])("preserves spaces inside one token when saving (compact: %s)", async (compact) => {
    const user = userEvent.setup();
    const { onSave } = renderTab({ custom_args: [] }, undefined, compact);

    await user.click(screen.getByRole("button", { name: /add argument/i }));
    await user.type(
      screen.getByRole("textbox", { name: /new argument/i }),
      "value with spaces",
    );
    await user.click(screen.getByRole("button", { name: /^add$/i }));

    expect(onSave).toHaveBeenCalledWith({ custom_args: ["value with spaces"] });
  });

  it("preserves a failed edit for correction and retries without losing saved arguments", async () => {
    const user = userEvent.setup();
    const onSave = vi.fn().mockRejectedValueOnce(new Error("Save failed")).mockResolvedValue(undefined);
    const { onDirtyChange } = renderTab({}, onSave);
    await user.click(screen.getByRole("button", { name: /edit argument 1/i }));
    const input = screen.getByRole("textbox", { name: /argument 1/i });
    await user.clear(input);
    await user.type(input, "--retry");
    await user.click(screen.getByRole("button", { name: /^update$/i }));

    expect(input).toHaveValue("--retry");
    expect(toast.error).toHaveBeenCalledWith("Save failed");
    expect(screen.getByText("research")).toBeInTheDocument();
    expect(onDirtyChange).toHaveBeenLastCalledWith(true);
    await user.click(screen.getByRole("button", { name: /^update$/i }));
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
    expect(screen.getByText("--retry")).toBeInTheDocument();
    expect(onSave).toHaveBeenLastCalledWith({ custom_args: ["--retry", "research"] });
    expect(onDirtyChange).toHaveBeenLastCalledWith(false);
  });

  it("persists deletion once and disables other mutations until it finishes", async () => {
    let finishSave!: () => void;
    const onSave = vi.fn(() => new Promise<void>((resolve) => { finishSave = resolve; }));
    renderTab({}, onSave);

    const remove = screen.getByRole("button", { name: /remove argument 1/i });
    fireEvent.click(remove);
    fireEvent.click(remove);
    expect(onSave).toHaveBeenCalledTimes(1);
    expect(onSave).toHaveBeenCalledWith({ custom_args: ["research"] });
    expect(screen.getByText("--profile")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /add argument/i })).toBeDisabled();
    expect(screen.getByRole("button", { name: /edit argument 2/i })).toBeDisabled();
    expect(remove).toBeDisabled();

    await act(async () => finishSave());
    await waitFor(() => expect(screen.queryByText("--profile")).not.toBeInTheDocument());
    expect(screen.getByRole("button", { name: /add argument/i })).toBeEnabled();
  });

  it("keeps a saved argument visible when deletion fails", async () => {
    const user = userEvent.setup();
    renderTab({}, vi.fn().mockRejectedValue(new Error("Delete failed")));
    await user.click(screen.getByRole("button", { name: /remove argument 1/i }));
    expect(screen.getByText("--profile")).toBeInTheDocument();
    expect(toast.error).toHaveBeenCalledWith("Delete failed");
    expect(screen.getByRole("button", { name: /remove argument 1/i })).toBeEnabled();
  });
});
