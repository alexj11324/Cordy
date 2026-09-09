"use client"

import { useEffect, useMemo, useState } from "react"
import { Badge } from "@orvilo/ui/components/reui/badge"
import {
  dataGridFeatures,
  DataGrid as ReuiDataGrid,
} from "@orvilo/ui/components/reui/data-grid/data-grid"
import type { DataGridI18nOverrides } from "@orvilo/ui/components/reui/data-grid/data-grid-i18n"
import { DataGridPagination } from "@orvilo/ui/components/reui/data-grid/data-grid-pagination"
import { DataGridScrollArea } from "@orvilo/ui/components/reui/data-grid/data-grid-scroll-area"
import { DataGridTable } from "@orvilo/ui/components/reui/data-grid/data-grid-table"
import {
  Frame,
  FrameDescription,
  FrameFooter,
  FrameHeader,
  FramePanel,
  FrameTitle,
} from "@orvilo/ui/components/reui/frame"
import {
  type PaginationState,
  type RowSelectionState,
  type SortingState,
  useTable,
  type ColumnVisibilityState,
} from "@tanstack/react-table"
import { toast } from "sonner"

import { Button } from "@orvilo/ui/components/ui/button"
import { Checkbox } from "@orvilo/ui/components/ui/checkbox"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@orvilo/ui/components/ui/dropdown-menu"
import {
  InputGroup,
  InputGroupAddon,
  InputGroupButton,
  InputGroupInput,
} from "@orvilo/ui/components/ui/input-group"
import { Label } from "@orvilo/ui/components/ui/label"
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@orvilo/ui/components/ui/popover"
import { Separator } from "@orvilo/ui/components/ui/separator"
import { TooltipProvider } from "@orvilo/ui/components/ui/tooltip"
import { IconPlaceholder } from "@orvilo/ui/components/ui/icon-placeholder"

import { getColumns, StatusBadge } from "./columns"
import {
  DATA_GRID_BASE_1_COPY,
  type DataGridBase1Actions,
  type DataGridBase1Copy,
} from "./copy"
import {
  EMPLOYEES,
  getBalanceHintText,
  STATUS_ORDER,
  type IEmployee,
  type Status,
} from "./data"

function employeeSearchBlob(e: IEmployee): string {
  const parts = [
    e.id,
    e.name,
    e.email,
    e.company,
    e.role,
    e.location,
    e.joined,
    e.status,
    String(e.balance),
    e.customerId,
    e.department,
    e.tenureLabel,
    e.timezone,
    e.companyTier,
    e.lastActiveLabel,
    e.balanceMeta?.reversal?.reason,
    e.balanceMeta?.reversal?.settledOn,
    getBalanceHintText(e) ?? "",
  ]
  return parts.filter(Boolean).join(" ").toLowerCase()
}

// ── Toolbar ──

interface ToolbarProps {
  searchQuery: string
  onSearchChange: (v: string) => void
  selectedStatuses: Status[]
  onStatusChange: (checked: boolean, status: Status) => void
  onClearFilters: () => void
  hasActiveFilters: boolean
  statusCounts: Record<string, number>
  copy: DataGridBase1Copy
}

function Toolbar({
  searchQuery,
  onSearchChange,
  selectedStatuses,
  onStatusChange,
  onClearFilters,
  hasActiveFilters,
  statusCounts,
  copy,
}: ToolbarProps) {
  return (
    <div className="flex flex-wrap items-center justify-between gap-3">
      {/* Actions */}
      <div className="flex flex-wrap items-center gap-2">
        <InputGroup className="w-full min-w-48 sm:w-48">
          <InputGroupAddon align="inline-start">
            <IconPlaceholder
              lucide="SearchIcon"
              tabler="IconSearch"
              hugeicons="Search01Icon"
              phosphor="MagnifyingGlassIcon"
              remixicon="RiSearchLine"
              aria-hidden="true"
            />
          </InputGroupAddon>
          <InputGroupInput
            placeholder={copy.searchPlaceholder}
            aria-label={copy.searchAria}
            value={searchQuery}
            onChange={(e) => onSearchChange(e.target.value)}
          />
          {searchQuery.length > 0 && (
            <InputGroupAddon align="inline-end">
              <InputGroupButton
                aria-label={copy.clearSearch}
                size="icon-xs"
                onClick={() => onSearchChange("")}
              >
                <IconPlaceholder
                  lucide="XIcon"
                  tabler="IconX"
                  hugeicons="Cancel01Icon"
                  phosphor="XIcon"
                  remixicon="RiCloseLine"
                  aria-hidden="true"
                />
              </InputGroupButton>
            </InputGroupAddon>
          )}
        </InputGroup>

        <Popover>
          <PopoverTrigger
            render={
              <Button variant="outline" aria-label={copy.filterStatusAria}>
                <IconPlaceholder
                  lucide="FilterIcon"
                  tabler="IconFilter"
                  hugeicons="FilterIcon"
                  phosphor="FunnelIcon"
                  remixicon="RiFilterLine"
                  aria-hidden="true"
                />
                {copy.status}
                {selectedStatuses.length > 0 && (
                  <Badge size="sm" variant="info-outline">
                    {selectedStatuses.length}
                  </Badge>
                )}
              </Button>
            }
          />
          <PopoverContent
            align="start"
            className="flex w-40 flex-col gap-2.5 p-3"
          >
            <span className="text-muted-foreground text-xs font-medium">
              {copy.filterByStatus}
            </span>
            {STATUS_ORDER.map((status) => (
              <div key={status} className="flex items-center gap-2.5">
                <Checkbox
                  id={`status-${status}`}
                  checked={selectedStatuses.includes(status)}
                  onCheckedChange={(checked) =>
                    onStatusChange(checked === true, status)
                  }
                />
                <Label
                  htmlFor={`status-${status}`}
                  className="flex min-w-0 flex-1 cursor-pointer items-center justify-between gap-2 font-normal"
                >
                  <StatusBadge status={status} copy={copy} />
                  <span className="text-muted-foreground shrink-0 text-xs tabular-nums">
                    {statusCounts[status] ?? 0}
                  </span>
                </Label>
              </div>
            ))}
          </PopoverContent>
        </Popover>

        {hasActiveFilters && (
          <Button
            type="button"
            variant="ghost"
            size="sm"
            className="text-muted-foreground"
            onClick={onClearFilters}
          >
            {copy.clearFilters}
          </Button>
        )}
      </div>

      <DropdownMenu>
        <DropdownMenuTrigger
          render={
            <Button variant="outline" aria-label={copy.tableActionsAria}>
              <IconPlaceholder
                lucide="MoreHorizontalIcon"
                tabler="IconDots"
                hugeicons="MoreHorizontalCircle01Icon"
                phosphor="DotsThreeIcon"
                remixicon="RiMoreLine"
                aria-hidden="true"
              />
              {copy.actions}
            </Button>
          }
        />
        <DropdownMenuContent align="end" className="w-36">
          <DropdownMenuGroup>
            <DropdownMenuItem
              onClick={() =>
                toast.success("Export ready", {
                  description: "Wire this to your API. Demo only.",
                })
              }
            >
              <IconPlaceholder
                lucide="FileDownIcon"
                tabler="IconDownload"
                hugeicons="Download01Icon"
                phosphor="DownloadSimpleIcon"
                remixicon="RiDownloadLine"
                aria-hidden="true"
              />
              {copy.exportCsv}
            </DropdownMenuItem>
            <DropdownMenuItem
              onClick={() =>
                toast.message("Refreshed", {
                  description: "Demo. Connect to live data when integrating.",
                })
              }
            >
              <IconPlaceholder
                lucide="RefreshCwIcon"
                tabler="IconRefresh"
                hugeicons="RefreshIcon"
                phosphor="ArrowsClockwiseIcon"
                remixicon="RiRefreshLine"
                aria-hidden="true"
              />
              {copy.refresh}
            </DropdownMenuItem>
            <DropdownMenuItem
              onClick={() =>
                toast.info("View settings", {
                  description:
                    "Open column layout and density in your app shell.",
                })
              }
            >
              <IconPlaceholder
                lucide="SettingsIcon"
                tabler="IconSettings"
                hugeicons="SettingsIcon"
                phosphor="GearIcon"
                remixicon="RiSettings3Line"
                aria-hidden="true"
              />
              {copy.viewSettings}
            </DropdownMenuItem>
          </DropdownMenuGroup>
        </DropdownMenuContent>
      </DropdownMenu>
    </div>
  )
}

// ── Main component ──

export { DATA_GRID_BASE_1_COPY, type DataGridBase1Copy, type DataGridBase1Actions } from "./copy"
export type { IEmployee, Status, Availability } from "./data"

export function DataGridView({
  data = EMPLOYEES,
  copy = DATA_GRID_BASE_1_COPY,
  actions = {},
  isLoading = false,
  selectedIds,
  onSelectedIdsChange,
  i18n,
  hiddenColumns,
}: {
  data?: IEmployee[]
  copy?: DataGridBase1Copy
  actions?: DataGridBase1Actions
  isLoading?: boolean
  selectedIds?: ReadonlySet<string>
  onSelectedIdsChange?: (ids: ReadonlySet<string>) => void
  i18n?: DataGridI18nOverrides
  hiddenColumns?: string[]
}) {
  const employees = data
  const columns = useMemo(() => getColumns(copy, actions), [actions, copy])
  const [pagination, setPagination] = useState<PaginationState>({
    pageIndex: 0,
    pageSize: 5,
  })
  const [sorting, setSorting] = useState<SortingState>([
    { id: "name", desc: false },
  ])
  const [searchQuery, setSearchQuery] = useState("")
  const [selectedStatuses, setSelectedStatuses] = useState<Status[]>([])

  const [columnOrder, setColumnOrder] = useState<string[]>(() =>
    getColumns().map((c) => c.id as string)
  )
  const [columnVisibility, setColumnVisibility] =
    useState<ColumnVisibilityState>(() => {
      const hidden = hiddenColumns ?? ["joined"]
      return Object.fromEntries(hidden.map((id) => [id, false]))
    })
  const [uncontrolledSelection, setUncontrolledSelection] =
    useState<RowSelectionState>({})
  const rowSelection = useMemo<RowSelectionState>(() => {
    if (!selectedIds) return uncontrolledSelection
    const next: RowSelectionState = {}
    for (const id of selectedIds) next[id] = true
    return next
  }, [selectedIds, uncontrolledSelection])
  const setRowSelection = (
    updater: RowSelectionState | ((old: RowSelectionState) => RowSelectionState)
  ) => {
    const next = typeof updater === "function" ? updater(rowSelection) : updater
    if (onSelectedIdsChange) {
      onSelectedIdsChange(
        new Set(Object.keys(next).filter((id) => next[id]))
      )
      return
    }
    setUncontrolledSelection(next)
  }

  const statusCounts = useMemo(
    () =>
      employees.reduce(
        (acc, e) => {
          acc[e.status] = (acc[e.status] || 0) + 1
          return acc
        },
        {} as Record<string, number>
      ),
    [employees]
  )

  const filteredData = useMemo(() => {
    return employees.filter((item) => {
      const matchesStatus =
        !selectedStatuses.length || selectedStatuses.includes(item.status)
      const searchLower = searchQuery.toLowerCase()
      const matchesSearch =
        !searchQuery || employeeSearchBlob(item).includes(searchLower)
      return matchesStatus && matchesSearch
    })
  }, [employees, searchQuery, selectedStatuses])

  const hasActiveFilters =
    searchQuery.trim().length > 0 || selectedStatuses.length > 0

  const handleStatusChange = (checked: boolean, status: Status) => {
    setSelectedStatuses((prev) =>
      checked ? [...prev, status] : prev.filter((s) => s !== status)
    )
  }

  const handleClearFilters = () => {
    setSelectedStatuses([])
    setSearchQuery("")
  }

  useEffect(() => {
    setPagination((current) =>
      current.pageIndex === 0 ? current : { ...current, pageIndex: 0 }
    )
  }, [searchQuery, selectedStatuses])

  const table = useTable({
    features: dataGridFeatures,
    columns,
    data: filteredData,
    pageCount: Math.ceil(filteredData.length / pagination.pageSize),
    getRowId: (row) => row.id,
    state: { pagination, sorting, columnOrder, columnVisibility, rowSelection },
    enableRowSelection: true,
    autoResetPageIndex: false,
    onColumnOrderChange: setColumnOrder,
    onColumnVisibilityChange: setColumnVisibility,
    onPaginationChange: setPagination,
    onRowSelectionChange: setRowSelection,
    onSortingChange: setSorting,
  })

  return (
    <TooltipProvider delay={200}>
      {/* Table */}
      <ReuiDataGrid
        table={table}
        recordCount={filteredData.length}
        i18n={i18n}
        isLoading={isLoading}
        emptyMessage={
          filteredData.length === 0 ? copy.empty : undefined
        }
        tableLayout={{
          columnsPinnable: true,
          columnsResizable: true,
          columnsMovable: true,
          columnsVisibility: true,
          dense: true,
        }}
      >
        <Frame variant="default" spacing="sm" className="w-full">
          <FrameHeader className="flex-row items-center justify-between gap-3">
            <div className="flex flex-col gap-0.5">
              <FrameTitle id="page-heading" className="text-balance">
                {copy.title}
              </FrameTitle>
              <FrameDescription className="text-xs text-pretty">
                {copy.description
                  .replace("{filtered}", String(filteredData.length))
                  .replace("{total}", String(employees.length))}
              </FrameDescription>
            </div>
            <Button
              type="button"
              onClick={() =>
                actions.onAdd
                  ? actions.onAdd()
                  : toast.info("Add customer", {
                      description:
                        "Connect your CRM or signup flow. Demo only.",
                    })
              }
            >
              <IconPlaceholder
                lucide="UserPlusIcon"
                tabler="IconUserPlus"
                hugeicons="UserAdd01Icon"
                phosphor="UserPlusIcon"
                remixicon="RiUserAddLine"
                aria-hidden="true"
              />
              {copy.add}
            </Button>
          </FrameHeader>
          <FramePanel className="bg-card p-0! shadow-none!">
            <div className="px-4 py-3">
              <Toolbar
                searchQuery={searchQuery}
                onSearchChange={setSearchQuery}
                selectedStatuses={selectedStatuses}
                onStatusChange={handleStatusChange}
                onClearFilters={handleClearFilters}
                hasActiveFilters={hasActiveFilters}
                statusCounts={statusCounts}
                copy={copy}
              />
            </div>
            <Separator />
            <DataGridScrollArea>
              <DataGridTable />
            </DataGridScrollArea>
          </FramePanel>
          <FrameFooter>
            <DataGridPagination />
          </FrameFooter>
        </Frame>
      </ReuiDataGrid>
    </TooltipProvider>
  )
}
