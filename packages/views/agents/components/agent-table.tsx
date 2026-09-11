"use client";

import { useMemo } from "react";
import type { TFunction } from "i18next";
import { Bot } from "lucide-react";
import { useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import {
  effectiveAccessScope,
  isAgentRuntimeBound,
} from "@orvilo/core/agents";
import { api } from "@orvilo/core/api";
import { useWorkspaceId } from "@orvilo/core/hooks";
import { useWorkspacePaths } from "@orvilo/core/paths";
import {
  deviceKind,
  deviceLabelForViewer,
  type LocalMachineMatch,
} from "@orvilo/core/runtimes";
import { workspaceKeys } from "@orvilo/core/workspace/queries";
import { resolvePublicFileUrl } from "@orvilo/core/workspace/avatar-url";
import { matchesPinyin } from "../../editor/extensions/pinyin-match";
import {
  DataGridView,
  DATA_GRID_BASE_1_COPY,
  type DataGridBase1Copy,
  type IEmployee,
  type Availability,
  type Status,
} from "@orvilo/ui/components/blocks/data-grid-base-1/components/data-grid-view";
import {
  Avatar,
  AvatarFallback,
  AvatarImage,
} from "@orvilo/ui/components/ui/avatar";
import { ProviderLogo } from "../../runtimes/components/provider-logo";
import { useT } from "../../i18n";
import { useNavigation } from "../../navigation";
import type { AgentListRow } from "./agents-page";
import { DeviceKindIcon } from "./device-kind-icon";

interface AgentTableProps {
  rows: AgentListRow[];
  selectedIds: ReadonlySet<string>;
  onSelectedIdsChange: (ids: ReadonlySet<string>) => void;
  noMatchText: string;
  locale: string;
  localDaemonId?: string | null;
  localMachineName?: string | null;
  currentUserId?: string | null;
}

/**
 * Agents table is ReUI `@reui/data-grid-base-1` as shipped. Only copy and
 * row data are swapped.
 */
export function AgentTable({
  rows,
  selectedIds,
  onSelectedIdsChange,
  noMatchText,
  locale,
  localDaemonId,
  localMachineName,
  currentUserId,
}: AgentTableProps) {
  const { t } = useT("agents");
  const { t: tCommon } = useT("common");
  const navigation = useNavigation();
  const paths = useWorkspacePaths();
  const wsId = useWorkspaceId();
  const qc = useQueryClient();

  const copy = useMemo<DataGridBase1Copy>(
    () => ({
      ...DATA_GRID_BASE_1_COPY,
      title: t(($) => $.page.title),
      description: t(($) => $.grid.description),
      searchPlaceholder: t(($) => $.page.search_placeholder),
      searchAria: t(($) => $.page.search_placeholder),
      clearSearch: t(($) => $.grid.clear_search),
      filterStatusAria: t(($) => $.grid.filter_status),
      status: t(($) => $.columns.status),
      filterByStatus: t(($) => $.grid.filter_status),
      clearFilters: t(($) => $.grid.clear_filters),
      tableActionsAria: t(($) => $.page.actions_aria),
      actions: t(($) => $.grid.actions),
      exportCsv: t(($) => $.grid.export_csv),
      refresh: t(($) => $.grid.refresh),
      viewSettings: t(($) => $.grid.view_settings),
      add: t(($) => $.page.new_agent),
      empty: noMatchText,
      customer: t(($) => $.columns.agent),
      company: t(($) => $.columns.device),
      role: t(($) => $.columns.model),
      location: t(($) => $.columns.owner),
      joined: t(($) => $.columns.created),
      balance: t(($) => $.columns.runs),
      rowActionsAria: t(($) => $.row.actions_aria),
      viewDetails: t(($) => $.grid.view_details),
      edit: t(($) => $.grid.edit),
      copyId: t(($) => $.grid.copy_id),
      delete: t(($) => $.row_actions.archive),
      restore: t(($) => $.row_actions.restore),
      deleteTitle: t(($) => $.grid.delete_title),
      deleteDescriptionBefore: t(($) => $.grid.delete_before),
      deleteDescriptionAfter: t(($) => $.grid.delete_after),
      cancel: tCommon(($) => $.cancel),
      statusActive: t(($) => $.availability.online),
      statusBlocked: t(($) => $.availability.archived),
      statusInactive: t(($) => $.availability.offline),
      statusPending: t(($) => $.row.needs_device),
      statusUnstable: t(($) => $.availability.unstable),
      customerIdCopied: t(($) => $.grid.id_copied),
      reversalOnFile: DATA_GRID_BASE_1_COPY.reversalOnFile,
      balanceInsight: DATA_GRID_BASE_1_COPY.balanceInsight,
      trend: DATA_GRID_BASE_1_COPY.trend,
    }),
    [noMatchText, t, tCommon],
  );

  const data = useMemo(
    () =>
      rows.map((row) =>
        toEmployee(row, t as TFunction<"agents">, locale, {
          localDaemonId,
          localMachineName,
          currentUserId,
        }),
      ),
    [currentUserId, locale, localDaemonId, localMachineName, rows, t],
  );

  return (
    <div className="min-h-0 flex-1 overflow-auto">
      <div className="mx-auto w-full max-w-7xl p-8 pt-12">
        <DataGridView
          actions={{
            onAdd: () => navigation.push(paths.newAgent()),
            onView: (id) => navigation.push(paths.agentDetail(id)),
            onEdit: (id) => navigation.push(paths.agentDetail(id)),
            onDelete: async (id) => {
              try {
                await api.archiveAgent(id);
                qc.invalidateQueries({ queryKey: workspaceKeys.agents(wsId) });
                toast.success(t(($) => $.row_actions.agent_archived_toast));
              } catch (error) {
                toast.error(
                  error instanceof Error
                    ? error.message
                    : t(($) => $.row_actions.archive_failed_toast),
                );
              }
            },
            onRestore: async (id) => {
              try {
                await api.restoreAgent(id);
                qc.invalidateQueries({ queryKey: workspaceKeys.agents(wsId) });
                toast.success(t(($) => $.row_actions.agent_restored_toast));
              } catch (error) {
                toast.error(
                  error instanceof Error
                    ? error.message
                    : t(($) => $.row_actions.restore_failed_toast),
                );
              }
            },
          }}
          copy={copy}
          data={data}
          hiddenColumns={["joined", "status"]}
          onSelectedIdsChange={onSelectedIdsChange}
          selectedIds={selectedIds}
          searchPredicate={(employee, query) =>
            matchesPinyin(employee.name, query) ||
            matchesPinyin(employee.email, query)
          }
          i18n={{
            labels: {
              sortAscending: t(($) => $.grid.sort_ascending),
              sortDescending: t(($) => $.grid.sort_descending),
              pinColumnStart: t(($) => $.grid.pin_column_start),
              pinColumnEnd: t(($) => $.grid.pin_column_end),
              moveColumnStart: t(($) => $.grid.move_column_start),
              moveColumnEnd: t(($) => $.grid.move_column_end),
              columnsMenu: t(($) => $.grid.columns_menu),
              unpinColumn: (title) =>
                t(($) => $.grid.unpin_column, { title }),
              toggleColumns: t(($) => $.grid.toggle_columns),
              rowCreate: t(($) => $.page.new_agent),
              pinRow: t(($) => $.grid.pin_row),
              unpinRow: t(($) => $.grid.unpin_row),
              selectRow: t(($) => $.grid.select_row),
              selectAll: t(($) => $.grid.select_all),
              expandRow: t(($) => $.grid.expand_row),
              collapseRow: t(($) => $.grid.collapse_row),
              dragToReorder: t(($) => $.grid.drag_to_reorder),
              dragToReorderRow: t(($) => $.grid.drag_to_reorder_row),
              reorderingUnavailable: t(
                ($) => $.grid.reordering_unavailable,
              ),
              loading: t(($) => $.grid.loading),
              empty: noMatchText,
              allRowsLoaded: t(($) => $.grid.all_rows_loaded),
              rowsPerPage: t(($) => $.grid.rows_per_page),
              paginationInfo: ({ from, to, count }) =>
                t(($) => $.grid.pagination_info)
                  .replace("{from}", String(from))
                  .replace("{to}", String(to))
                  .replace("{count}", String(count)),
              previousPage: t(($) => $.grid.previous_page),
              nextPage: t(($) => $.grid.next_page),
              goToPage: (page) => t(($) => $.grid.go_to_page, { page }),
              paginationEllipsis: t(($) => $.grid.pagination_ellipsis),
              filterSelectedCount: (count) =>
                t(($) => $.grid.filter_selected_count, { count }),
              filterNoResults: t(($) => $.grid.filter_no_results),
              filterClear: t(($) => $.grid.filter_clear),
            },
          }}
        />
      </div>
    </div>
  );
}

export function AgentTableSkeleton() {
  return (
    <div className="min-h-0 flex-1 overflow-auto">
      <div className="mx-auto w-full max-w-7xl p-8 pt-12" aria-busy="true">
        <DataGridView data={[]} isLoading />
      </div>
    </div>
  );
}

export function AccessCell({ row }: { row: AgentListRow }) {
  const { t } = useT("agents");
  const scope = useMemo(
    () =>
      effectiveAccessScope(
        row.agent.permission_mode,
        row.agent.invocation_targets,
      ),
    [row.agent.permission_mode, row.agent.invocation_targets],
  );
  const label = t(($) =>
    scope === "workspace"
      ? $.access.scope_labels.workspace
      : scope === "specific-people"
        ? $.access.scope_labels.specific_people
        : $.access.scope_labels.owner_only,
  );
  return (
    <span className="text-foreground min-w-0 truncate font-medium">{label}</span>
  );
}

function toEmployee(
  row: AgentListRow,
  t: TFunction<"agents">,
  locale: string,
  localMachine: LocalMachineMatch,
): IEmployee {
  const { agent, runtime, owner, runCount, lastActiveDays } = row;
  const scope = effectiveAccessScope(
    agent.permission_mode,
    agent.invocation_targets,
  );
  const accessLabel = t(($) =>
    scope === "workspace"
      ? $.access.scope_labels.workspace
      : scope === "specific-people"
        ? $.access.scope_labels.specific_people
        : $.access.scope_labels.owner_only,
  );
  const tenure =
    lastActiveDays === null
      ? t(($) => $.last_active.none_short)
      : lastActiveDays === 0
        ? t(($) => $.last_active.today)
        : t(($) => $.last_active.days_ago, { count: lastActiveDays });
  const ownerName = owner?.name ?? agent.owner_id?.slice(0, 8) ?? "—";
  const ownerAvatarSrc = owner?.avatar_url
    ? resolvePublicFileUrl(owner.avatar_url)
    : null;
  const ownerInitials = ownerName
    .split(" ")
    .map((part) => part[0])
    .join("")
    .slice(0, 2);

  return {
    id: agent.id,
    name: agent.name,
    availability: toAvailability(row),
    avatar: "",
    avatarMedia: runtime ? (
      <ProviderLogo className="size-5" provider={runtime.provider} />
    ) : (
      <Bot className="size-5" />
    ),
    status: toStatus(row),
    flag: "us",
    email: agent.description?.trim() || t(($) => $.row.no_description),
    company: runtime
      ? deviceLabelForViewer(
          runtime,
          localMachine,
          t(($) => $.gallery_card.device_this_machine),
        )
      : t(($) => $.row.needs_device),
    companyLogo: runtime ? (
      <DeviceKindIcon kind={deviceKind(runtime)} />
    ) : null,
    role: agent.model || "—",
    department: accessLabel,
    joined: new Date(agent.created_at).toLocaleDateString(locale),
    tenureLabel: tenure,
    location: ownerName,
    locationMedia: (
      <Avatar className="size-8">
        {ownerAvatarSrc ? <AvatarImage alt="" src={ownerAvatarSrc} /> : null}
        <AvatarFallback>{ownerInitials || "?"}</AvatarFallback>
      </Avatar>
    ),
    balance: runCount,
    balanceLabel: String(runCount),
    joinedTimestamp: Date.parse(agent.created_at),
    canManage: row.canManage,
    isArchived: Boolean(agent.archived_at),
    isSystemAgent: Boolean(agent.system_key),
    customerId: agent.id,
    lastActiveLabel: tenure,
  };
}

function toStatus(row: AgentListRow): Status {
  if (row.agent.archived_at) return "Blocked";
  if (!isAgentRuntimeBound(row.agent)) return "Pending";
  if (row.presence?.availability === "online") return "Active";
  if (row.presence?.availability === "unstable") return "Unstable";
  return "Inactive";
}

function toAvailability(row: AgentListRow): Availability {
  if (row.agent.archived_at) return "offline";
  if (row.presence?.availability === "online") return "online";
  if (row.presence?.availability === "unstable") return "away";
  return "offline";
}
