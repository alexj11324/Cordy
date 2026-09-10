import { fireEvent, render, screen } from "@testing-library/react";
import { useTable, type ColumnDef } from "@tanstack/react-table";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { MemberRole } from "@orvilo/core/types";
import { renderWithI18n } from "../../test/i18n";
import { NavigationProvider } from "../../navigation";
import { TeamDirectoryPage } from "./team-directory-page";

import {
  DataGrid,
  dataGridFeatures,
} from "@orvilo/ui/components/reui/data-grid/data-grid";
import { DataGridScrollArea } from "@orvilo/ui/components/reui/data-grid/data-grid-scroll-area";
import { DataGridTable } from "@orvilo/ui/components/reui/data-grid/data-grid-table";

const directory = vi.hoisted(() => ({
  members: [
    {
      id: "member-1",
      workspace_id: "workspace-1",
      user_id: "user-1",
      role: "owner" as MemberRole,
      created_at: "2026-04-17T12:00:00.000Z",
      name: "Ada Lovelace",
      email: "ada@example.com",
      avatar_url: null,
    },
  ],
  invitations: [
    {
      id: "invitation-1",
      invitee_email: "grace@example.com",
      role: "member",
      status: "pending",
      inviter_id: "user-1",
      inviter_name: "Ada Lovelace",
      created_at: "2026-05-02T12:00:00.000Z",
    },
  ],
}));

vi.mock("@tanstack/react-query", () => ({
  useQuery: ({ queryKey }: { queryKey: string[] }) => ({
    data: queryKey[0] === "members"
      ? directory.members
      : queryKey[0] === "invitations" ? directory.invitations : undefined,
    isLoading: false,
  }),
  useMutation: () => ({ mutate: vi.fn(), isPending: false }),
  useQueryClient: () => ({ invalidateQueries: vi.fn() }),
}));
vi.mock("@orvilo/core/workspace/queries", () => ({
  memberListOptions: () => ({ queryKey: ["members"] }),
  invitationListOptions: () => ({ queryKey: ["invitations"] }),
  workspaceKeys: {},
}));
vi.mock("@orvilo/core/paths", () => ({
  useCurrentWorkspace: () => ({ id: "workspace-1" }),
  useWorkspacePaths: () => ({ agentTeams: () => "/workspace-1/agent-teams" }),
}));
vi.mock("@orvilo/core/auth", () => ({
  useAuthStore: (selector: (state: { user: { id: string } }) => unknown) =>
    selector({ user: { id: "user-1" } }),
}));
vi.mock("@orvilo/core/billing", () => ({
  usePreviewWorkspaceSeatPurchase: () => ({}),
  usePurchaseWorkspaceSeats: () => ({}),
  workspaceSubscriptionSummaryOptions: () => ({ queryKey: ["subscription"] }),
}));

function renderDirectory() {
  return renderWithI18n(
    <NavigationProvider value={{
      push: vi.fn(),
      replace: vi.fn(),
      back: vi.fn(),
      pathname: "/workspace-1/team",
      searchParams: new URLSearchParams(),
      hash: "",
      getShareableUrl: (path) => path,
    }}>
      <TeamDirectoryPage />
    </NavigationProvider>,
  );
}

beforeEach(() => {
  directory.members[0]!.role = "owner";
});

type DirectoryRow = {
  id: string;
  fullName: string;
  email: string;
};

const columns: ColumnDef<typeof dataGridFeatures, DirectoryRow>[] = [
  {
    accessorKey: "fullName",
    header: "Name",
  },
  {
    accessorKey: "email",
    header: "Email",
  },
];

function DirectoryGridFixture() {
  const table = useTable({
    features: dataGridFeatures,
    data: [
      {
        id: "ada",
        fullName: "Ada Lovelace",
        email: "ada@example.com",
      },
    ],
    columns,
    getRowId: (row) => row.id,
    initialState: { pagination: { pageIndex: 0, pageSize: 5 } },
  });

  return (
    <DataGrid table={table} recordCount={1} tableLayout={{ width: "fixed" }}>
      <DataGridScrollArea>
        <DataGridTable />
      </DataGridScrollArea>
    </DataGrid>
  );
}

describe("Team directory grid", () => {
  it("keeps tabs, search, view settings, and a single invitation entry on the flat list", () => {
    const { container } = renderDirectory();

    expect(container.querySelector('[data-slot="card"]')).toBeNull();
    expect(screen.getByRole("tab", { name: /Members\s*1/ })).toHaveAttribute("aria-selected", "true");
    expect(screen.getByRole("link", { name: "Agent teams" })).toHaveAttribute("href", "/workspace-1/agent-teams");
    expect(screen.getByRole("button", { name: "View settings" })).toBeEnabled();
    expect(screen.queryByRole("button", { name: "Add member" })).not.toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: "Invite people" })).toHaveLength(1);

    fireEvent.change(screen.getByRole("textbox", { name: "Search members…" }), { target: { value: "missing" } });
    expect(screen.queryByRole("cell", { name: /Ada Lovelace/ })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Clear search" }));
    expect(screen.getByRole("cell", { name: /Ada Lovelace/ })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("tab", { name: /Invitations\s*1/ }));
    expect(screen.getByRole("textbox", { name: "Search invitations…" })).toBeInTheDocument();
    expect(screen.getByRole("cell", { name: /grace@example.com/ })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Invite people" }));
    expect(screen.getByRole("dialog", { name: "Invite people" })).toBeInTheDocument();
  });

  it("keeps invitation management disabled for ordinary members", () => {
    directory.members[0]!.role = "member";
    renderDirectory();

    expect(screen.getByRole("button", { name: "Invite people" })).toBeDisabled();
    expect(screen.getByRole("tab", { name: /Invitations\s*1/ })).toBeEnabled();
  });

  it("renders the Atlas data grid through the native v9 table instance", () => {
    render(<DirectoryGridFixture />);

    expect(
      document.querySelector('[data-slot="data-grid-table"]'),
    ).toBeTruthy();
    expect(
      screen.getByRole("columnheader", { name: "Name" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("cell", { name: "Ada Lovelace" }),
    ).toBeInTheDocument();
  });
});
