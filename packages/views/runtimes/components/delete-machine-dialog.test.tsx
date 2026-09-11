// @vitest-environment jsdom

import { describe, it, expect, vi, beforeEach } from "vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import type { Agent, AgentRuntime } from "@orvilo/core/types";
import { I18nProvider } from "@orvilo/core/i18n/react";
import enCommon from "../../locales/en/common.json";
import enRuntimes from "../../locales/en/runtimes.json";
import type { RuntimeMachine } from "./runtime-machines";

const TEST_RESOURCES = {
  en: { common: enCommon, runtimes: enRuntimes },
};

const { ApiError, apiDeleteRuntime, apiUnbindAgentsAndDeleteRuntime } =
  vi.hoisted(() => {
    class ApiError extends Error {
      status: number;
      body: unknown;
      constructor(message: string, status: number, body: unknown) {
        super(message);
        this.status = status;
        this.body = body;
      }
    }
    return {
      ApiError,
      apiDeleteRuntime: vi.fn(),
      apiUnbindAgentsAndDeleteRuntime: vi.fn(),
    };
  });

vi.mock("@orvilo/core/api", () => ({
  api: {
    deleteRuntime: (...args: unknown[]) => apiDeleteRuntime(...args),
    unbindAgentsAndDeleteRuntime: (...args: unknown[]) =>
      apiUnbindAgentsAndDeleteRuntime(...args),
    listAgents: vi.fn(),
  },
  ApiError,
}));

vi.mock("@orvilo/core/runtimes/mutations", () => ({
  useDeleteRuntime: () => ({
    isPending: false,
    mutate: vi.fn(),
    mutateAsync: (...args: unknown[]) => apiDeleteRuntime(...args),
  }),
  useUnbindAgentsAndDeleteRuntime: () => ({
    isPending: false,
    mutate: vi.fn(),
    mutateAsync: (vars: {
      runtimeId: string;
      expectedActiveAgentIds: string[];
    }) =>
      apiUnbindAgentsAndDeleteRuntime(
        vars.runtimeId,
        vars.expectedActiveAgentIds,
      ),
  }),
}));

vi.mock("@tanstack/react-query", async () => {
  const actual =
    await vi.importActual<typeof import("@tanstack/react-query")>(
      "@tanstack/react-query",
    );
  return {
    ...actual,
    useQuery: vi.fn(() => ({ data: [], isLoading: false })),
  };
});

vi.mock("sonner", () => ({
  toast: { error: vi.fn(), success: vi.fn() },
}));

import { useQuery } from "@tanstack/react-query";
import { toast } from "sonner";
import {
  canDeleteRuntimeMachine,
  DeleteMachineDialog,
} from "./delete-machine-dialog";

const mockedUseQuery = vi.mocked(useQuery);

function makeRuntime(overrides: Partial<AgentRuntime> = {}): AgentRuntime {
  return {
    id: "rt-1",
    workspace_id: "ws-1",
    daemon_id: "daemon-1",
    name: "Claude (MacBook)",
    runtime_mode: "local",
    provider: "claude",
    launch_header: "",
    status: "offline",
    device_info: "MacBook",
    metadata: {},
    owner_id: "user-me",
    visibility: "private",
    last_seen_at: null,
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
    ...overrides,
  };
}

function pendingRuntime(
  overrides: Partial<AgentRuntime> = {},
): AgentRuntime {
  return makeRuntime({
    id: "pending-runtime-profile:profile-1",
    status: "offline",
    metadata: { pending_custom_runtime: true },
    ...overrides,
  });
}

function makeAgent(id: string, overrides: Partial<Agent> = {}): Agent {
  return {
    id,
    workspace_id: "ws-1",
    runtime_id: "rt-1",
    name: `Agent ${id}`,
    description: "",
    instructions: "",
    avatar_url: null,
    runtime_mode: "local",
    runtime_config: {},
    custom_args: [],
    visibility: "private",
    permission_mode: "private",
    invocation_targets: [],
    status: "idle",
    max_concurrent_tasks: 1,
    model: "claude-sonnet-4-5",
    owner_id: "user-me",
    skills: [],
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
    archived_at: null,
    archived_by: null,
    ...overrides,
  };
}

function makeMachine(
  overrides: Partial<RuntimeMachine> = {},
): RuntimeMachine {
  const runtimes = overrides.runtimes ?? [makeRuntime()];
  return {
    id: "machine-1",
    daemonId: "daemon-1",
    title: "MacBook",
    subtitle: null,
    deviceInfo: "MacBook",
    cliVersion: null,
    launchedBy: null,
    mode: "local",
    section: "local",
    isCurrent: false,
    health: "offline",
    runtimes,
    onlineCount: 0,
    issueCount: 0,
    runningCount: 0,
    queuedCount: 0,
    providerNames: ["claude"],
    lastSeenAt: null,
    ...overrides,
  };
}

function renderDialog(
  opts: {
    machine?: RuntimeMachine;
    cachedAgents?: Agent[];
    onStopLocalDaemon?: () => Promise<void>;
    onDeleted?: () => void;
  } = {},
) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const onOpenChange = vi.fn();
  const onDeleted = opts.onDeleted ?? vi.fn();

  mockedUseQuery.mockImplementation(((queryArg: unknown) => {
    const q = queryArg as { queryKey?: readonly unknown[] };
    const key = q?.queryKey ?? [];
    const tail = key[key.length - 1];
    if (tail === "agents") {
      return {
        data: opts.cachedAgents ?? [],
        isLoading: false,
      } as unknown as ReturnType<typeof useQuery>;
    }
    return { data: [], isLoading: false } as unknown as ReturnType<
      typeof useQuery
    >;
  }) as unknown as typeof useQuery);

  const utils = render(
    <I18nProvider locale="en" resources={TEST_RESOURCES}>
      <QueryClientProvider client={qc}>
        <DeleteMachineDialog
          open
          onOpenChange={onOpenChange}
          machine={opts.machine ?? makeMachine()}
          wsId="ws-1"
          onStopLocalDaemon={opts.onStopLocalDaemon}
          onDeleted={onDeleted}
        />
      </QueryClientProvider>
    </I18nProvider>,
  );
  return { ...utils, onOpenChange, onDeleted };
}

async function confirmDelete() {
  fireEvent.click(screen.getByRole("checkbox"));
  const submit = screen.getByRole("button", { name: "Delete machine" });
  await waitFor(() => expect(submit).not.toBeDisabled());
  fireEvent.click(submit);
}

describe("canDeleteRuntimeMachine", () => {
  it("hides delete when every runtime is still pending", () => {
    expect(
      canDeleteRuntimeMachine(makeMachine({ runtimes: [pendingRuntime()] }), {
        isAdmin: true,
        currentUserId: "user-me",
      }),
    ).toBe(false);
  });

  it("lets an admin delete when at least one runtime is registered", () => {
    expect(
      canDeleteRuntimeMachine(
        makeMachine({
          runtimes: [pendingRuntime(), makeRuntime({ owner_id: "other" })],
        }),
        { isAdmin: true, currentUserId: "user-me" },
      ),
    ).toBe(true);
  });

  it("ignores pending ownership for non-admins", () => {
    expect(
      canDeleteRuntimeMachine(
        makeMachine({
          runtimes: [
            pendingRuntime({ owner_id: "user-me" }),
            makeRuntime({ id: "rt-2", owner_id: "other" }),
          ],
        }),
        { isAdmin: false, currentUserId: "user-me" },
      ),
    ).toBe(false);
  });

  it("lets the owner delete a registered runtime they own", () => {
    expect(
      canDeleteRuntimeMachine(makeMachine(), {
        isAdmin: false,
        currentUserId: "user-me",
      }),
    ).toBe(true);
  });
});

describe("DeleteMachineDialog", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("deletes each registered runtime and skips pending ones", async () => {
    apiDeleteRuntime.mockResolvedValue({ status: "ok" });
    const onDeleted = vi.fn();
    renderDialog({
      machine: makeMachine({
        runtimes: [
          makeRuntime({ id: "rt-1" }),
          pendingRuntime(),
          makeRuntime({ id: "rt-2" }),
        ],
      }),
      onDeleted,
    });

    await confirmDelete();

    await waitFor(() => expect(apiDeleteRuntime).toHaveBeenCalledTimes(2));
    expect(apiDeleteRuntime).toHaveBeenCalledWith("rt-1");
    expect(apiDeleteRuntime).toHaveBeenCalledWith("rt-2");
    expect(toast.success).toHaveBeenCalledWith("Machine deleted");
    expect(onDeleted).toHaveBeenCalled();
  });

  it("unbinds agents before deleting a bound runtime", async () => {
    apiUnbindAgentsAndDeleteRuntime.mockResolvedValueOnce({
      status: "ok",
      agents_unbound: 1,
      tasks_cancelled: 0,
    });
    renderDialog({
      cachedAgents: [makeAgent("a-1")],
    });

    await confirmDelete();

    await waitFor(() =>
      expect(apiUnbindAgentsAndDeleteRuntime).toHaveBeenCalledWith("rt-1", [
        "a-1",
      ]),
    );
    expect(apiDeleteRuntime).not.toHaveBeenCalled();
  });

  it("stops the local daemon before deleting this computer", async () => {
    const order: string[] = [];
    apiDeleteRuntime.mockImplementation(async () => {
      order.push("delete");
      return { status: "ok" };
    });
    const onStopLocalDaemon = vi.fn(async () => {
      order.push("stop");
    });

    renderDialog({
      machine: makeMachine({ isCurrent: true }),
      onStopLocalDaemon,
    });
    expect(
      screen.getByText(/local daemon on this computer will be stopped/i),
    ).toBeInTheDocument();
    expect(
      screen.queryByText(/may re-register itself/i),
    ).not.toBeInTheDocument();

    await confirmDelete();

    await waitFor(() => expect(onStopLocalDaemon).toHaveBeenCalled());
    expect(order).toEqual(["stop", "delete"]);
  });

  it("does not delete when stopping the local daemon fails", async () => {
    const onDeleted = vi.fn();
    renderDialog({
      machine: makeMachine({ isCurrent: true }),
      onStopLocalDaemon: async () => {
        throw new Error("daemon still running");
      },
      onDeleted,
    });

    await confirmDelete();

    await waitFor(() =>
      expect(toast.error).toHaveBeenCalledWith(
        "Couldn't delete this machine",
      ),
    );
    expect(apiDeleteRuntime).not.toHaveBeenCalled();
    expect(onDeleted).not.toHaveBeenCalled();
  });

  it("warns that a remote daemon may self-heal", () => {
    renderDialog({
      machine: makeMachine({
        isCurrent: false,
        runtimes: [makeRuntime({ runtime_mode: "local", status: "online" })],
      }),
    });
    expect(
      screen.getByText(/may re-register itself/i),
    ).toBeInTheDocument();
    expect(
      screen.queryByText(/local daemon on this computer will be stopped/i),
    ).not.toBeInTheDocument();
  });

  it("skips profile-instance refusals and still finishes the rest", async () => {
    apiDeleteRuntime
      .mockRejectedValueOnce(
        new ApiError("conflict", 409, {
          code: "runtime_profile_instance_delete_unsupported",
        }),
      )
      .mockResolvedValueOnce({ status: "ok" });
    const onDeleted = vi.fn();

    renderDialog({
      machine: makeMachine({
        runtimes: [
          makeRuntime({ id: "rt-profile" }),
          makeRuntime({ id: "rt-2" }),
        ],
      }),
      onDeleted,
    });

    await confirmDelete();

    await waitFor(() => expect(onDeleted).toHaveBeenCalled());
    expect(apiDeleteRuntime).toHaveBeenCalledTimes(2);
    expect(toast.success).toHaveBeenCalledWith("Machine deleted");
  });

  it("does not toast success when there is nothing registered to delete", async () => {
    const onDeleted = vi.fn();
    renderDialog({
      machine: makeMachine({ runtimes: [pendingRuntime()] }),
      onDeleted,
    });

    await confirmDelete();

    await waitFor(() =>
      expect(toast.error).toHaveBeenCalledWith(
        "Couldn't delete this machine",
      ),
    );
    expect(apiDeleteRuntime).not.toHaveBeenCalled();
    expect(onDeleted).not.toHaveBeenCalled();
    expect(toast.success).not.toHaveBeenCalled();
  });
});
