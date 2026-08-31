import { useImperativeHandle, useRef, useState } from "react";
import { describe, it, expect, vi, beforeEach } from "vitest";
import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { AutomationTrigger } from "@patchbay/core/types";
import { renderWithI18n } from "../../test/i18n";

// Regression cover for PB-5649: editing a manual-only automation (no triggers)
// showed the schedule panel seeded with the editor's default — 09:00 every day
// — as if that were the automation's schedule. Saving compared that default
// against itself, found no change, and wrote nothing, while the toast said the
// automation was updated and the detail page still read "No triggers
// configured". The panel now states what is true (no schedule) and asks for the
// schedule to be added explicitly; adding one writes it, default or not.

const mockUpdateAutomation = vi.hoisted(() => vi.fn());
const mockCreateTrigger = vi.hoisted(() => vi.fn());
const mockUpdateTrigger = vi.hoisted(() => vi.fn());

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
  useCreateAutomation: () => ({ mutateAsync: vi.fn() }),
  useCreateAutomationTrigger: () => ({ mutateAsync: mockCreateTrigger }),
  useUpdateAutomation: () => ({ mutateAsync: mockUpdateAutomation }),
  useUpdateAutomationTrigger: () => ({ mutateAsync: mockUpdateTrigger }),
}));

vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn() } }));

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

vi.mock("./pickers/timezone-picker", () => ({
  TimezonePicker: ({ value }: { value: string }) => <div data-testid="timezone-picker">{value}</div>,
}));

import { AutomationDialog } from "./automation-dialog";

const AUTOMATION_ID = "ap-1";

function trigger(overrides: Partial<AutomationTrigger> = {}): AutomationTrigger {
  return {
    id: "trg-1",
    automation_id: AUTOMATION_ID,
    kind: "schedule",
    enabled: true,
    cron_expression: "TZ=Asia/Shanghai 30 8 * * *",
    timezone: "Asia/Shanghai",
    next_run_at: null,
    webhook_token: null,
    label: null,
    last_fired_at: null,
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
    ...overrides,
  };
}

function renderEditDialog(triggers: AutomationTrigger[]) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return renderWithI18n(
    <QueryClientProvider client={qc}>
      <AutomationDialog
        mode="edit"
        open
        onOpenChange={vi.fn()}
        automationId={AUTOMATION_ID}
        initial={{
          title: "Issue title sweep",
          description: "",
          project_id: null,
          executor_type: "agent",
          executor_id: "agent-1",
          execution_mode: "run_only",
          subscriber_user_ids: [],
        }}
        triggers={triggers}
        collaborators={[]}
        canManageAccess={false}
      />
    </QueryClientProvider>,
  );
}

const saveButton = () => screen.getByRole("button", { name: "Save" });
const addScheduleButton = () => screen.getByRole("button", { name: "Add schedule" });

describe("AutomationDialog schedule section on an automation with no schedule", () => {
  beforeEach(() => {
    mockUpdateAutomation.mockReset().mockResolvedValue({ id: AUTOMATION_ID });
    mockCreateTrigger.mockReset().mockResolvedValue({ id: "trg-new" });
    mockUpdateTrigger.mockReset().mockResolvedValue({ id: "trg-1" });
  });

  it("says the automation has no schedule instead of showing a fabricated one", () => {
    renderEditDialog([]);

    expect(
      screen.getByText("No schedule — this automation only runs when triggered manually."),
    ).toBeInTheDocument();
    expect(addScheduleButton()).toBeInTheDocument();
    // The editor — and with it the next-run preview that reads as a promise —
    // stays out of sight until the user asks for a schedule, and the footer
    // stops promising automatic runs the automation will not make.
    expect(screen.queryByTestId("timezone-picker")).not.toBeInTheDocument();
    expect(
      screen.queryByText("Once saved, runs automatically until paused."),
    ).not.toBeInTheDocument();
  });

  it("restores the auto-run hint once a schedule is added", async () => {
    const user = userEvent.setup();
    renderEditDialog([]);

    await user.click(addScheduleButton());

    expect(
      await screen.findByText("Once saved, runs automatically until paused."),
    ).toBeInTheDocument();
  });

  it("writes the schedule on save even when the user keeps the default 09:00", async () => {
    const user = userEvent.setup();
    renderEditDialog([]);

    await user.click(addScheduleButton());
    expect(await screen.findByTestId("timezone-picker")).toBeInTheDocument();

    await user.click(saveButton());

    await waitFor(() => expect(mockCreateTrigger).toHaveBeenCalledTimes(1));
    expect(mockCreateTrigger.mock.calls[0]?.[0]).toMatchObject({
      automationId: AUTOMATION_ID,
      kind: "schedule",
    });
    expect(mockCreateTrigger.mock.calls[0]?.[0].cron_expression).toMatch(/(^|\s)0 9 \* \* \*$/);
    expect(mockUpdateTrigger).not.toHaveBeenCalled();
  });

  it("leaves the automation manual when the user only edits other fields", async () => {
    const user = userEvent.setup();
    renderEditDialog([]);

    await user.type(screen.getByLabelText("title"), " v2");
    await user.click(saveButton());

    await waitFor(() => expect(mockUpdateAutomation).toHaveBeenCalledTimes(1));
    expect(mockCreateTrigger).not.toHaveBeenCalled();
    expect(mockUpdateTrigger).not.toHaveBeenCalled();
  });

  it("never patches a cron into a non-schedule trigger", async () => {
    const user = userEvent.setup();
    // An api-kind trigger is not a schedule: the panel must offer to create one
    // rather than treat that row as the schedule it is about to overwrite.
    renderEditDialog([trigger({ kind: "api", cron_expression: null, timezone: null })]);

    await user.click(addScheduleButton());
    await user.click(saveButton());

    await waitFor(() => expect(mockCreateTrigger).toHaveBeenCalledTimes(1));
    expect(mockUpdateTrigger).not.toHaveBeenCalled();
  });
});

describe("AutomationDialog schedule section on an automation that has one", () => {
  beforeEach(() => {
    mockUpdateAutomation.mockReset().mockResolvedValue({ id: AUTOMATION_ID });
    mockCreateTrigger.mockReset().mockResolvedValue({ id: "trg-new" });
    mockUpdateTrigger.mockReset().mockResolvedValue({ id: "trg-1" });
  });

  it("shows the stored schedule, not the empty state", () => {
    renderEditDialog([trigger()]);

    expect(screen.queryByRole("button", { name: "Add schedule" })).not.toBeInTheDocument();
    expect(screen.getByTestId("timezone-picker")).toHaveTextContent("Asia/Shanghai");
  });

  it("does not rewrite a schedule the user never changed", async () => {
    const user = userEvent.setup();
    renderEditDialog([trigger()]);

    await user.click(saveButton());

    await waitFor(() => expect(mockUpdateAutomation).toHaveBeenCalledTimes(1));
    expect(mockUpdateTrigger).not.toHaveBeenCalled();
    expect(mockCreateTrigger).not.toHaveBeenCalled();
  });
});
