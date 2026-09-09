"use client"

import { memo, useState } from "react"
import { useCopyToClipboard } from "@orvilo/ui/hooks/use-copy-to-clipboard"
import { Badge } from "@orvilo/ui/components/reui/badge"
import { type DataGridFeatures } from "@orvilo/ui/components/reui/data-grid/data-grid"
import { DataGridColumnHeader } from "@orvilo/ui/components/reui/data-grid/data-grid-column-header"
import {
  DataGridTableRowSelect,
  DataGridTableRowSelectAll,
} from "@orvilo/ui/components/reui/data-grid/data-grid-table"
import type { ColumnDef, Row } from "@tanstack/react-table"
import { toast } from "sonner"

import { cn } from "@orvilo/ui/lib/utils"
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@orvilo/ui/components/ui/alert-dialog"
import {
  Avatar,
  AvatarFallback,
  AvatarImage,
} from "@orvilo/ui/components/ui/avatar"
import { Button } from "@orvilo/ui/components/ui/button"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@orvilo/ui/components/ui/dropdown-menu"
import { Item, ItemMedia } from "@orvilo/ui/components/reui/item"
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@orvilo/ui/components/ui/tooltip"
import { IconPlaceholder } from "@orvilo/ui/components/ui/icon-placeholder"

import {
  DATA_GRID_BASE_1_COPY,
  type DataGridBase1Actions,
  type DataGridBase1Copy,
} from "./copy"
import { getBalanceHintText, type IEmployee, type Status } from "./data"

const currencyCompact = new Intl.NumberFormat("en-US", {
  style: "currency",
  currency: "USD",
  minimumFractionDigits: 2,
  maximumFractionDigits: 2,
})

const availabilityColor: Record<string, string> = {
  online: "bg-green-500",
  away: "bg-yellow-400",
  busy: "bg-red-500",
  offline: "bg-gray-500",
}

export const StatusBadge = memo(function StatusBadge({
  status,
  copy = DATA_GRID_BASE_1_COPY,
}: {
  status: Status
  copy?: DataGridBase1Copy
}) {
  if (status === "Active")
    return <Badge variant="success-outline">{copy.statusActive}</Badge>
  if (status === "Blocked")
    return <Badge variant="destructive-outline">{copy.statusBlocked}</Badge>
  if (status === "Unstable")
    return <Badge variant="warning-outline">{copy.statusUnstable}</Badge>
  if (status === "Inactive")
    return <Badge variant="info-outline">{copy.statusInactive}</Badge>
  return <Badge variant="warning-outline">{copy.statusPending}</Badge>
})

const BalanceCell = memo(function BalanceCell({
  row,
  copy = DATA_GRID_BASE_1_COPY,
}: {
  row: Row<DataGridFeatures, IEmployee>
  copy?: DataGridBase1Copy
}) {
  const meta = row.original.balanceMeta
  const formatted =
    row.original.balanceLabel ?? currencyCompact.format(row.original.balance)
  const trend = meta?.trendPct
  const up = trend !== undefined && trend >= 0
  const hint = getBalanceHintText(row.original)

  return (
    <div className="flex min-w-0 flex-col gap-0.5">
      {/* Actions */}
      <div className="flex items-center gap-1.5">
        <span className="text-foreground font-medium tabular-nums">
          {formatted}
        </span>
        {meta?.reversal && (
          <Tooltip>
            <TooltipTrigger
              render={
                <button
                  type="button"
                  className="text-destructive hover:text-destructive/90 focus-visible:ring-ring focus-visible:ring-offset-background inline-flex shrink-0 rounded-sm focus-visible:ring-2 focus-visible:ring-offset-2"
                  aria-label={`Reversal ${currencyCompact.format(meta.reversal.amount)}. ${meta.reversal.reason}`}
                />
              }
            >
              <IconPlaceholder
                lucide="AlertCircleIcon"
                tabler="IconAlertCircle"
                hugeicons="AlertCircleIcon"
                phosphor="WarningCircleIcon"
                remixicon="RiErrorWarningLine"
                className="size-3.5"
                aria-hidden="true"
              />
            </TooltipTrigger>
            <TooltipContent side="top" className="max-w-xs p-3">
              <div className="flex flex-col gap-2">
                <div className="flex items-center justify-between gap-2">
                  <span className="font-medium">{copy.reversalOnFile}</span>
                  <Badge variant="destructive" size="xs">
                    {currencyCompact.format(meta.reversal.amount)}
                  </Badge>
                </div>
                <div className="flex flex-col gap-1 text-xs opacity-80">
                  <p>• {meta.reversal.reason}</p>
                  <p>• Settled {meta.reversal.settledOn}</p>
                </div>
              </div>
            </TooltipContent>
          </Tooltip>
        )}
      </div>
      {trend !== undefined &&
        (hint ? (
          <Tooltip>
            <TooltipTrigger
              render={
                <button
                  type="button"
                  className="text-muted-foreground hover:text-foreground focus-visible:ring-ring focus-visible:ring-offset-background inline-flex shrink-0 rounded-sm focus-visible:ring-2 focus-visible:ring-offset-2"
                  aria-label={`Balance insight: ${hint}`}
                />
              }
            >
              <IconPlaceholder
                lucide="InfoIcon"
                tabler="IconInfoCircle"
                hugeicons="InformationCircleIcon"
                phosphor="InfoIcon"
                remixicon="RiInformationLine"
                className="size-3.5"
                aria-hidden="true"
              />
            </TooltipTrigger>
            <TooltipContent side="top" className="max-w-xs p-3">
              <div className="flex flex-col gap-2">
                <div className="flex items-center justify-between gap-2">
                  <span className="font-medium">{copy.balanceInsight}</span>
                  <Badge variant="info-outline" size="xs">
                    {copy.trend}
                  </Badge>
                </div>
                <div className="flex flex-col gap-1 text-xs opacity-80">
                  <p>{hint}</p>
                </div>
              </div>
            </TooltipContent>
          </Tooltip>
        ) : (
          <div
            className={cn(
              "flex items-center gap-1 text-xs tabular-nums",
              up ? "text-success" : "text-destructive"
            )}
          >
            {up ? (
              <IconPlaceholder
                lucide="ArrowUpIcon"
                tabler="IconArrowUp"
                hugeicons="ArrowUp02Icon"
                phosphor="ArrowUpIcon"
                remixicon="RiArrowUpLine"
                className="size-3.5 shrink-0"
                aria-hidden="true"
              />
            ) : (
              <IconPlaceholder
                lucide="ArrowDownIcon"
                tabler="IconArrowDown"
                hugeicons="ArrowDown02Icon"
                phosphor="ArrowDownIcon"
                remixicon="RiArrowDownLine"
                className="size-3.5 shrink-0"
                aria-hidden="true"
              />
            )}
            <span>{Math.abs(trend).toFixed(1)}%</span>
          </div>
        ))}
    </div>
  )
})

const CustomerCell = memo(function CustomerCell({
  row,
}: {
  row: Row<DataGridFeatures, IEmployee>
}) {
  const o = row.original

  return (
    <div className="flex items-center gap-2">
      <div className="relative shrink-0">
        <Avatar className="size-8">
          {o.avatarMedia ? (
            <AvatarFallback>{o.avatarMedia}</AvatarFallback>
          ) : (
            <>
              <AvatarImage src={o.avatar} alt="" />
              <AvatarFallback>
                {o.name
                  .split(" ")
                  .map((n) => n[0])
                  .join("")}
              </AvatarFallback>
            </>
          )}
        </Avatar>
        <span
          className={cn(
            "ring-background absolute right-0 bottom-0.5 size-2 rounded-full ring-2",
            availabilityColor[o.availability]
          )}
          aria-hidden
        />
      </div>
      <div className="min-w-0">
        <div className="text-foreground line-clamp-1 font-medium">{o.name}</div>
        <div
          className="text-muted-foreground line-clamp-1 text-xs"
          title={o.email}
        >
          {o.email}
        </div>
      </div>
    </div>
  )
})

export interface DataGridRowActionState {
  canEdit: boolean
  canArchive: boolean
  canRestore: boolean
}

export function getRowActionState(
  row: Pick<IEmployee, "canManage" | "isArchived" | "isSystemAgent">,
  actions?: DataGridBase1Actions,
): DataGridRowActionState {
  const canManage = row.canManage ?? true
  const isArchived = row.isArchived ?? false
  const isSystemAgent = row.isSystemAgent ?? false
  return {
    canEdit: canManage,
    canArchive: canManage && !isArchived && !isSystemAgent && !!actions?.onDelete,
    canRestore: canManage && isArchived && !!actions?.onRestore,
  }
}

export function ActionsCell({
  row,
  copy = DATA_GRID_BASE_1_COPY,
  actions,
}: {
  row: Row<DataGridFeatures, IEmployee>
  copy?: DataGridBase1Copy
  actions?: DataGridBase1Actions
}) {
  const { copyToClipboard } = useCopyToClipboard()
  const [deleteOpen, setDeleteOpen] = useState(false)
  const { canEdit, canArchive, canRestore } = getRowActionState(
    row.original,
    actions,
  )

  const handleCopyId = () => {
    copyToClipboard(row.original.id)
    toast.success(copy.customerIdCopied, { description: row.original.id })
  }

  const handleDeleteConfirm = () => {
    setDeleteOpen(false)
    if (actions?.onDelete) {
      actions.onDelete(row.original.id)
      return
    }
    toast.message("Delete requested", {
      description: `${row.original.name}. Wire to your API (demo).`,
    })
  }

  return (
    <>
      <DropdownMenu>
        <DropdownMenuTrigger
          render={
            <Button
              size="icon"
              variant="ghost"
              className="size-7"
              aria-label={copy.rowActionsAria}
            />
          }
        >
          <IconPlaceholder
            lucide="MoreHorizontalIcon"
            tabler="IconDots"
            hugeicons="MoreHorizontalCircle01Icon"
            phosphor="DotsThreeIcon"
            remixicon="RiMoreLine"
            aria-hidden="true"
          />
        </DropdownMenuTrigger>
        <DropdownMenuContent side="bottom" align="start" className="w-44">
          <DropdownMenuGroup>
            <DropdownMenuItem
              onClick={() =>
                actions?.onView
                  ? actions.onView(row.original.id)
                  : toast.info("View profile", {
                      description: "Demo. Open detail route in your app.",
                    })
              }
            >
              <IconPlaceholder
                lucide="EyeIcon"
                tabler="IconEye"
                hugeicons="ViewIcon"
                phosphor="EyeIcon"
                remixicon="RiEyeLine"
                className="size-4"
                aria-hidden="true"
              />
              {copy.viewDetails}
            </DropdownMenuItem>
            {canEdit && (
              <DropdownMenuItem
                onClick={() =>
                  actions?.onEdit
                    ? actions.onEdit(row.original.id)
                    : toast.info("Edit customer", {
                        description: "Demo. Open your edit form.",
                      })
                }
              >
                <IconPlaceholder
                  lucide="PencilIcon"
                  tabler="IconPencil"
                  hugeicons="PenIcon"
                  phosphor="PencilIcon"
                  remixicon="RiPencilLine"
                  className="size-4"
                  aria-hidden="true"
                />
                {copy.edit}
              </DropdownMenuItem>
            )}
            <DropdownMenuItem onClick={handleCopyId}>
              <IconPlaceholder
                lucide="CopyIcon"
                tabler="IconCopy"
                hugeicons="Copy01Icon"
                phosphor="CopyIcon"
                remixicon="RiFileCopyLine"
                className="size-4"
                aria-hidden="true"
              />
              {copy.copyId}
            </DropdownMenuItem>
            {canRestore && (
              <DropdownMenuItem
                onClick={() => actions?.onRestore?.(row.original.id)}
              >
                <IconPlaceholder
                  lucide="RotateCcwIcon"
                  tabler="IconRestore"
                  hugeicons="RefreshIcon"
                  phosphor="ArrowCounterClockwiseIcon"
                  remixicon="RiRestartLine"
                  className="size-4"
                  aria-hidden="true"
                />
                {copy.restore}
              </DropdownMenuItem>
            )}
            {canArchive && (
              <>
                <DropdownMenuSeparator />
                <DropdownMenuItem
                  variant="destructive"
                  onClick={() => setDeleteOpen(true)}
                >
                  <IconPlaceholder
                    lucide="Trash2Icon"
                    tabler="IconTrash"
                    hugeicons="Delete02Icon"
                    phosphor="TrashIcon"
                    remixicon="RiDeleteBinLine"
                    className="size-4"
                    aria-hidden="true"
                  />
                  {copy.delete}
                </DropdownMenuItem>
              </>
            )}
          </DropdownMenuGroup>
        </DropdownMenuContent>
      </DropdownMenu>

      <AlertDialog open={deleteOpen} onOpenChange={setDeleteOpen}>
        <AlertDialogContent size="sm">
          <AlertDialogHeader>
            <AlertDialogTitle>{copy.deleteTitle}</AlertDialogTitle>
            <AlertDialogDescription>
              {copy.deleteDescriptionBefore}{" "}
              <span className="text-foreground font-medium">
                {row.original.name}
              </span>{" "}
              {copy.deleteDescriptionAfter}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>{copy.cancel}</AlertDialogCancel>
            <AlertDialogAction
              variant="destructive"
              onClick={handleDeleteConfirm}
            >
              {copy.delete}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  )
}

export function getColumns(
  copy: DataGridBase1Copy = DATA_GRID_BASE_1_COPY,
  actions: DataGridBase1Actions = {}
): ColumnDef<DataGridFeatures, IEmployee>[] {
  return [

  {
    accessorKey: "id",
    id: "id",
    header: () => <DataGridTableRowSelectAll />,
    cell: ({ row }) => <DataGridTableRowSelect row={row} />,
    enableSorting: false,
    size: 35,
    enableResizing: false,
    enableHiding: false,
    meta: {
      headerClassName: "ps-4!",
      cellClassName: "ps-4!",
    },
  },
  {
    accessorKey: "name",
    id: "name",
    header: ({ column }) => (
      <DataGridColumnHeader column={column} visibility={true} />
    ),
    cell: ({ row }) => <CustomerCell row={row} />,
    size: 220,
    enableSorting: true,
    enableHiding: false,
    enableResizing: true,
    minSize: 200,
    meta: {
      headerTitle: copy.customer,
      autoSize: true,
    },
  },
  {
    accessorKey: "company",
    id: "company",
    header: ({ column }) => (
      <DataGridColumnHeader column={column} visibility={true} />
    ),
    cell: ({ row }) => (
      <div className="flex min-w-0 items-center gap-1">
        <Item render={<span />} className="w-auto shrink-0 border-0 p-0">
          <ItemMedia variant="icon" className="size-auto">
            {row.original.companyLogo}
          </ItemMedia>
        </Item>
        <span className="text-foreground min-w-0 truncate font-medium">
          {row.original.company}
        </span>
      </div>
    ),
    size: 150,
    enableSorting: true,
    enableHiding: true,
    enableResizing: true,
    meta: {
      headerTitle: copy.company,
    },
  },
  {
    accessorKey: "role",
    id: "role",
    header: ({ column }) => (
      <DataGridColumnHeader column={column} visibility={true} />
    ),
    cell: ({ row }) => (
      <div className="flex min-w-0 flex-col gap-0.5">
        <span className="text-foreground line-clamp-1 font-medium">
          {row.original.role}
        </span>
        {row.original.department && (
          <span className="text-muted-foreground line-clamp-1 text-xs">
            {row.original.department}
          </span>
        )}
      </div>
    ),
    size: 160,
    enableSorting: true,
    enableHiding: true,
    enableResizing: true,
    meta: {
      headerTitle: copy.role,
    },
  },
  {
    accessorKey: "location",
    id: "location",
    header: ({ column }) => (
      <DataGridColumnHeader column={column} visibility={true} />
    ),
    cell: ({ row }) => (
      <div className="flex min-w-0 flex-col gap-0.5">
        <div className="flex min-w-0 items-center gap-1.5">
          {row.original.locationMedia ?? (
            <img
              src={`https://flagcdn.com/${row.original.flag.toLowerCase()}.svg`}
              alt=""
              width={16}
              height={16}
              loading="lazy"
              decoding="async"
              className="size-4 shrink-0 rounded-full object-cover"
            />
          )}
          <span className="text-foreground line-clamp-1 font-medium">
            {row.original.location}
          </span>
        </div>
        {row.original.timezone && (
          <span className="text-muted-foreground line-clamp-1 text-xs tabular-nums">
            {row.original.timezone}
          </span>
        )}
      </div>
    ),
    size: 160,
    enableSorting: true,
    enableHiding: true,
    enableResizing: true,
    meta: {
      headerTitle: copy.location,
    },
  },
  {
    id: "joined",
    accessorFn: (row) => row.joinedTimestamp ?? 0,
    header: ({ column }) => (
      <DataGridColumnHeader column={column} visibility={true} />
    ),
    cell: ({ row }) => (
      <div className="flex min-w-0 flex-col gap-0.5">
        <span className="text-foreground line-clamp-1 font-medium">
          {row.original.joined}
        </span>
        {row.original.tenureLabel && (
          <span className="text-muted-foreground line-clamp-1 text-xs">
            {row.original.tenureLabel}
          </span>
        )}
      </div>
    ),
    size: 150,
    enableSorting: true,
    enableHiding: true,
    enableResizing: true,
    meta: {
      headerTitle: copy.joined,
    },
  },
  {
    accessorKey: "balance",
    id: "balance",
    header: ({ column }) => (
      <DataGridColumnHeader column={column} visibility={true} />
    ),
    cell: ({ row }) => <BalanceCell row={row} copy={copy} />,
    size: 180,
    enableSorting: true,
    enableHiding: true,
    enableResizing: true,
    meta: {
      headerTitle: copy.balance,
    },
  },
  {
    accessorKey: "status",
    id: "status",
    header: ({ column }) => (
      <DataGridColumnHeader column={column} visibility={true} />
    ),
    cell: ({ row }) => <StatusBadge status={row.original.status} copy={copy} />,
    size: 110,
    enableSorting: true,
    enableHiding: true,
    enableResizing: true,
    meta: {
      headerTitle: copy.status,
    },
  },
  {
    id: "actions",
    header: "",
    cell: ({ row }) => <ActionsCell row={row} copy={copy} actions={actions} />,
    size: 50,
    enableSorting: false,
    enableHiding: false,
    enableResizing: false,
  },
]
}

export const columns = getColumns()
