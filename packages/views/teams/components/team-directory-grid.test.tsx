import { render, screen } from "@testing-library/react";
import { useTable, type ColumnDef } from "@tanstack/react-table";
import { describe, expect, it } from "vitest";

import {
  DataGrid,
  dataGridFeatures,
} from "@orvilo/ui/components/reui/data-grid/data-grid";
import { DataGridScrollArea } from "@orvilo/ui/components/reui/data-grid/data-grid-scroll-area";
import { DataGridTable } from "@orvilo/ui/components/reui/data-grid/data-grid-table";

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
