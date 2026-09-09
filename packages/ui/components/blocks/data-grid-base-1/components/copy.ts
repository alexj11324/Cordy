export interface DataGridBase1Copy {
  title: string
  description: string
  searchPlaceholder: string
  searchAria: string
  clearSearch: string
  filterStatusAria: string
  status: string
  filterByStatus: string
  clearFilters: string
  tableActionsAria: string
  actions: string
  exportCsv: string
  refresh: string
  viewSettings: string
  add: string
  empty: string
  customer: string
  company: string
  role: string
  location: string
  joined: string
  balance: string
  rowActionsAria: string
  viewDetails: string
  edit: string
  copyId: string
  delete: string
  restore: string
  deleteTitle: string
  deleteDescriptionBefore: string
  deleteDescriptionAfter: string
  cancel: string
  statusActive: string
  statusBlocked: string
  statusInactive: string
  statusPending: string
  statusUnstable: string
  customerIdCopied: string
  reversalOnFile: string
  balanceInsight: string
  trend: string
}

/** Original English copy from `@reui/data-grid-base-1`. */
export const DATA_GRID_BASE_1_COPY: DataGridBase1Copy = {
  title: "Customers",
  description: "{filtered} of {total} customers",
  searchPlaceholder: "Search customers...",
  searchAria: "Search customers",
  clearSearch: "Clear search",
  filterStatusAria: "Filter by customer status",
  status: "Status",
  filterByStatus: "Filter by status",
  clearFilters: "Clear filters",
  tableActionsAria: "Table actions",
  actions: "Actions",
  exportCsv: "Export CSV",
  refresh: "Refresh",
  viewSettings: "View settings",
  add: "Add customer",
  empty:
    "No customers match your search or status filters. Clear filters or widen your search.",
  customer: "Customer",
  company: "Company",
  role: "Role",
  location: "Location",
  joined: "Joined",
  balance: "Balance ($)",
  rowActionsAria: "Row actions",
  viewDetails: "View Details",
  edit: "Edit",
  copyId: "Copy ID",
  delete: "Delete",
  restore: "Restore",
  deleteTitle: "Delete customer?",
  deleteDescriptionBefore: "This will remove",
  deleteDescriptionAfter:
    "from the list. This action cannot be undone in production. Connect your API to persist changes.",
  cancel: "Cancel",
  statusActive: "Active",
  statusBlocked: "Blocked",
  statusInactive: "Inactive",
  statusPending: "Pending",
  statusUnstable: "Unstable",
  customerIdCopied: "Customer ID copied",
  reversalOnFile: "Reversal on file",
  balanceInsight: "Balance insight",
  trend: "Trend",
}

export interface DataGridBase1Actions {
  onAdd?: () => void
  onView?: (id: string) => void
  onEdit?: (id: string) => void
  onDelete?: (id: string) => void
  onRestore?: (id: string) => void
}
