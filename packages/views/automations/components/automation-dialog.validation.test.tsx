import { useImperativeHandle, useRef, useState } from "react";
import { describe, it, expect, vi, beforeEach } from "vitest";
import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { renderWithI18n } from "../../test/i18n";

// Regression cover for GitHub #6231: "Create Automation" was disabled whenever
// a required field was empty, so a user who had not picked an executor saw a
// dead control and no reason for it. The button is now live whenever a save
// isn't already in flight, and the click it accepts is what surfaces an inline
// error on the field at fault.

const mockCreateAutomation = vi.hoisted(() => vi.fn());
const mockCreateTrigger = vi.hoisted(() => vi.fn());

vi.mock("@patchbay/core/hooks", () => ({ useWorkspaceId: () => "ws-test" }));
vi.mock("@patchbay/core/paths", () => ({ useCurrentWorkspace: () => ({ name: "Acme" }) }));

vi.mock("@patchbay/core/workspace/queries", () => ({
  agentListOptions: (wsId: string) => ({
    queryKey: ["agents", wsId],
    queryFn: async () => [
      {
        id: "agent-1",
        name: "Scout",
        description: "Researches things",
        archived_at: null,
        runtime_id: "runtime-1",
      },
    ],
  }),
  teamListOptions: (wsId: string) => ({
    queryKey: ["teams", wsId],
    queryFn: async () => [],
  }),
}));

vi.mock("@patchbay/core/projects/queries", () => ({
  projectListOptions: (wsId: string) => ({
    queryKey: ["projects", wsId],
    queryFn: async () => [],
  }),
}));

vi.mock("@patchbay/core/automations/queries", () => ({
  cronPreviewOptions: (wsId: string, expr: string, tz: string) => ({
    queryKey: ["cron-preview", wsId, expr, tz],
    queryFn: async () => ({ next_runs: ["2126-07-14T01:00:00Z"] }),
    retry: false,
  }),
}));

vi.mock("@patchbay/core/automations/mutations", () => ({
  useCreateAutomation: () => ({ mutateAsync: mockCreateAutomation }),
  useCreateAutomationTrigger: () => ({ mutateAsync: mockCreateTrigger }),
  useUpdateAutomation: () => ({ mutateAsync: vi.fn() }),
  useUpdateAutomationTrigger: () => ({ mutateAsync: vi.fn() }),
}));

vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn() } }));

// Tiptap in jsdom is neither cheap nor the subject here: the title is a plain
// input whose ref honours focus(), which is all the dialog asks of it.
vi.mock("../../editor", () => ({
  TitleEditor: ({ ref, defaultValue, placeholder, onChange, onSubmit }: any) => {
    const [value, setValue] = useState(defaultValue ?? "");
    const inputRef = useRef<HTMLInputElement>(null);
    useImperativeHandle(ref, () => ({
      getText: () => value,
      focus: () => inputRef.current?.focus(),
      focusAtCoords: () => inputRef.current?.focus(),
    }));
    return (
      <input
        ref={inputRef}
        aria-label="title"
        value={value}
        placeholder={placeholder}
        onChange={(e) => {
          setValue(e.target.value);
          onChange?.(e.target.value);
        }}
        onKeyDown={(e) => {
          if (e.key === "Enter") onSubmit?.();
        }}
      />
    );
  },
  ContentEditor: ({ placeholder }: any) => <textarea aria-label="runbook" placeholder={placeholder} />,
}));

vi.mock("../../common/actor-avatar", () => ({
  ActorAvatar: ({ actorId }: { actorId: string }) => <span data-testid="actor-avatar">{actorId}</span>,
}));

vi.mock("./subscriber-multi-select", () => ({
  SubscriberMultiSelect: () => <div data-testid="subscriber-multi-select" />,
}));

vi.mock("../../projects/components/project-picker", () => ({
  ProjectPicker: ({ triggerRender }: { triggerRender: React.ReactElement }) => triggerRender,
}));

vi.mock("../pickers/timezone-picker", () => ({
  TimezonePicker: ({ value }: { value: string }) => <div data-testid="timezone-picker">{value}</div>,
}));

import { AutomationDialog } from "./automation-dialog";

function renderCreateDialog() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return renderWithI18n(
    <QueryClientProvider client={qc}>
      <AutomationDialog mode="create" open onOpenChange={vi.fn()} />
    </QueryClientProvider>,
  );
}

const createButton = () => screen.getByRole("button", { name: "Create automation" });
const executorTrigger = () => screen.getByRole("button", { name: /Select agent or team/ });

describe("AutomationDialog required-field feedback", () => {
  beforeEach(() => {
    mockCreateAutomation.mockReset();
    mockCreateTrigger.mockReset();
  });

  it("leaves the create button fully live while required fields are empty", () => {
    renderCreateDialog();
    // A dimmed button is a dead end with no room for a reason. Neither the
    // native attribute nor the ARIA one may be set: the click is the whole
    // feedback channel.
    expect(createButton()).not.toBeDisabled();
    expect(createButton()).not.toHaveAttribute("aria-disabled");
  });

  it("names the missing title on a blocked submit instead of doing nothing", async () => {
    const user = userEvent.setup();
    renderCreateDialog();

    expect(screen.queryByText("Enter a name for this automation.")).not.toBeInTheDocument();
    await user.click(createButton());

    expect(await screen.findByText("Enter a name for this automation.")).toBeInTheDocument();
    expect(mockCreateAutomation).not.toHaveBeenCalled();
  });

  it("names the missing executor once the title is filled, and marks the picker invalid", async () => {
    const user = userEvent.setup();
    renderCreateDialog();

    await user.type(screen.getByLabelText("title"), "Daily digest");
    await user.click(createButton());

    expect(
      await screen.findByText("Choose the agent or team that will run this automation."),
    ).toBeInTheDocument();
    // The title error clears itself the moment the field is filled — no second
    // submit needed to retire an error the user has already fixed.
    expect(screen.queryByText("Enter a name for this automation.")).not.toBeInTheDocument();
    expect(executorTrigger()).toHaveAttribute("aria-invalid", "true");
    expect(mockCreateAutomation).not.toHaveBeenCalled();
  });

  it("clears the executor error and submits once an agent is picked", async () => {
    const user = userEvent.setup();
    mockCreateAutomation.mockResolvedValue({ id: "ap-1" });
    mockCreateTrigger.mockResolvedValue({ id: "tr-1" });
    renderCreateDialog();

    await user.type(screen.getByLabelText("title"), "Daily digest");
    await user.click(createButton());
    await screen.findByText("Choose the agent or team that will run this automation.");

    await user.click(executorTrigger());
    await user.click(await screen.findByRole("button", { name: /Scout/ }));

    await waitFor(() => {
      expect(
        screen.queryByText("Choose the agent or team that will run this automation."),
      ).not.toBeInTheDocument();
    });

    await user.click(createButton());
    await waitFor(() => expect(mockCreateAutomation).toHaveBeenCalledTimes(1));
    expect(mockCreateAutomation.mock.calls[0]?.[0]).toMatchObject({
      title: "Daily digest",
      executor_type: "agent",
      executor_id: "agent-1",
    });
  });
});
