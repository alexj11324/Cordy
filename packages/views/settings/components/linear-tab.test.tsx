// @vitest-environment jsdom

import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { renderWithI18n } from "../../test/i18n";

const connectionRef = vi.hoisted(() => ({
  current: {
    configured: true,
    connected: true,
    pull_import_enabled: false,
    push_enabled: true,
    connection: {
      id: "conn-1",
      workspace_id: "ws-1",
      organization_id: "org-1",
      organization_name: "Acme Linear",
      actor_id: "actor-1",
      scopes: [],
      webhook_id: null,
      status: "active",
      token_expires_at: "2099-01-01T00:00:00Z",
      last_success_at: "2026-09-01T00:00:00Z",
      last_error: null,
      created_at: "2026-01-01T00:00:00Z",
      updated_at: "2026-01-01T00:00:00Z",
    },
  },
}));

vi.mock("@orvilo/core/linear", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@orvilo/core/linear")>();
  return {
    ...actual,
    linearConnectionOptions: () => ({
      queryKey: ["linear-connection"],
      queryFn: async () => connectionRef.current,
    }),
    linearBindingsOptions: () => ({
      queryKey: ["linear-bindings"],
      queryFn: async () => ({ bindings: [] }),
    }),
    linearCatalogOptions: (_workspaceId: string, enabled = true) => ({
      queryKey: ["linear-catalog", enabled],
      queryFn: async () => ({ teams: [], projects: [], users: [], labels: [], workflow_states: [] }),
      enabled,
    }),
    linearMemberBindingsOptions: () => ({
      queryKey: ["linear-member-bindings"],
      queryFn: async () => ({ bindings: [] }),
    }),
    linearConflictsOptions: () => ({
      queryKey: ["linear-conflicts"],
      queryFn: async () => ({ conflicts: [] }),
    }),
  };
});

vi.mock("@orvilo/core/projects", () => ({
  projectListOptions: () => ({
    queryKey: ["projects"],
    queryFn: async () => ({ projects: [] }),
  }),
}));

vi.mock("@orvilo/core/workspace/queries", () => ({
  memberListOptions: () => ({
    queryKey: ["members"],
    queryFn: async () => [],
  }),
}));

vi.mock("@orvilo/core/api", () => ({
  ApiError: class ApiError extends Error {
    status: number;
    constructor(message: string, status = 500) {
      super(message);
      this.status = status;
    }
  },
  api: {
    connectLinear: vi.fn(),
    disconnectLinear: vi.fn(),
  },
}));

import { LinearIntegrationCard } from "./linear-tab";

function renderCard() {
  const qc = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return renderWithI18n(
    <QueryClientProvider client={qc}>
      <LinearIntegrationCard canManage isGuest={false} workspaceId="ws-1" />
    </QueryClientProvider>,
  );
}

afterEach(() => {
  cleanup();
  connectionRef.current.connection.status = "active";
  connectionRef.current.configured = true;
});

describe("LinearIntegrationCard", () => {
  it("uses the IM connection badge and kebab instead of Authorized / inline buttons", async () => {
    renderCard();

    await waitFor(() => {
      expect(screen.getByText("Connected")).toBeInTheDocument();
    });
    expect(screen.queryByText("Authorized")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Disconnect" })).not.toBeInTheDocument();
    expect(screen.queryByText("Acme Linear")).not.toBeInTheDocument();
    expect(screen.queryByText("Loading Linear catalog")).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Manage" }));
    expect(screen.getByRole("menuitem", { name: "Manage" })).toBeInTheDocument();
    expect(screen.getByRole("menuitem", { name: "Disconnect" })).toBeInTheDocument();
  });

  it("does not mount the catalog dialog until manage is chosen", async () => {
    renderCard();
    await waitFor(() => {
      expect(screen.getByText("Connected")).toBeInTheDocument();
    });
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });
});
