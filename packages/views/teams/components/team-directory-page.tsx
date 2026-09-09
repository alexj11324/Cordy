"use client";

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  useTable,
  type ColumnDef,
  type ColumnVisibilityState,
  type SortingState,
} from "@tanstack/react-table";
import { toast } from "sonner";
import {
  CheckIcon,
  Loader2Icon,
  MailPlusIcon,
  SearchIcon,
  Settings2Icon,
  XIcon,
} from "lucide-react";
import { api, errorCode } from "@orvilo/core/api";
import { useAuthStore } from "@orvilo/core/auth";
import {
  useCurrentWorkspace,
  useWorkspacePaths,
} from "@orvilo/core/paths";
import {
  usePreviewWorkspaceSeatPurchase,
  usePurchaseWorkspaceSeats,
  workspaceSubscriptionSummaryOptions,
} from "@orvilo/core/billing";
import {
  invitationListOptions,
  memberListOptions,
  workspaceKeys,
} from "@orvilo/core/workspace/queries";
import type {
  MemberRole,
  PurchaseWorkspaceSeatsRequest,
  WorkspaceSeatPurchasePreview,
} from "@orvilo/core/types";
import { Badge } from "@orvilo/ui/components/reui/badge";
import {
  DataGrid,
  dataGridFeatures,
  type DataGridFeatures,
} from "@orvilo/ui/components/reui/data-grid/data-grid";
import { DataGridPagination } from "@orvilo/ui/components/reui/data-grid/data-grid-pagination";
import { DataGridScrollArea } from "@orvilo/ui/components/reui/data-grid/data-grid-scroll-area";
import { DataGridTable } from "@orvilo/ui/components/reui/data-grid/data-grid-table";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@orvilo/ui/components/ui/alert-dialog";
import { Button } from "@orvilo/ui/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@orvilo/ui/components/ui/dialog";
import { Field, FieldGroup, FieldLabel } from "@orvilo/ui/components/ui/field";
import {
  InputGroup,
  InputGroupAddon,
  InputGroupButton,
  InputGroupInput,
} from "@orvilo/ui/components/ui/input-group";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@orvilo/ui/components/ui/popover";
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@orvilo/ui/components/ui/select";
import { Separator } from "@orvilo/ui/components/ui/separator";
import { Skeleton } from "@orvilo/ui/components/ui/skeleton";
import { Tabs, TabsList, TabsTrigger } from "@orvilo/ui/components/ui/tabs";
import {
  createInvitationGridColumns,
  createMemberGridColumns,
  INVITATION_TOGGLE_COLUMNS,
  MEMBER_TOGGLE_COLUMNS,
  type InvitationRowAction,
  type MemberRowAction,
} from "./team-directory-columns";
import {
  toDirectoryInvitation,
  toDirectoryMember,
  type DirectoryInvitation,
  type DirectoryMember,
} from "./team-directory-data";
import { useLocale, useT } from "../../i18n";
import { AppLink } from "../../navigation";
import { CollapsedNavTrigger } from "../../layout/page-header";
import { formatStripeMinorAmount } from "../../settings/components/billing-format";
import {
  isSingleSeatInvitePreview,
  purchasedSeatIsReadyForInvitation,
  seatInvitationCanRetryAfterPurchase,
  seatPurchaseCanRetryWithSameQuote,
  seatPurchaseMatchesPreview,
} from "../../settings/components/seat-invite-purchase";
import { classifyDirectoryInviteError } from "./team-directory-invite";

type DirectoryTab = "members" | "invitations";
type TableDensity = "comfortable" | "compact";
type TeamsTranslator = ReturnType<typeof useT<"teams">>["t"];

type DirectoryInviteSeatPurchase = {
  workspaceId: string;
  email: string;
  role: MemberRole;
  preview: WorkspaceSeatPurchasePreview;
  idempotencyKey: string;
  phase: "review" | "purchasing" | "waiting" | "inviting" | "error";
  submittedAt?: number;
  error?: string;
  retryable?: boolean;
};

const PAGE_SIZES = [5, 10, 20] as const;
const SEAT_PURCHASE_CONFIRM_TIMEOUT_MS = 2 * 60_000;

function createSeatPurchaseKey(workspaceId: string): string {
  const suffix =
    globalThis.crypto?.randomUUID?.() ??
    `${Date.now()}-${Math.random().toString(36).slice(2)}`;
  return `invite-seat-${workspaceId}-${suffix}`.slice(0, 200);
}

function roleLabel(role: MemberRole, t: TeamsTranslator): string {
  if (role === "owner") return t(($) => $.directory.owner_role);
  if (role === "admin") return t(($) => $.directory.admin_role);
  return t(($) => $.directory.member_role);
}

function TabCount({ active, count }: { active: boolean; count: number }) {
  return (
    <Badge
      variant={active ? "primary-light" : "outline"}
      radius="full"
      className="tabular-nums"
    >
      {count}
    </Badge>
  );
}

function DirectorySkeleton() {
  return (
    <div className="min-w-0 w-full">
      <div className="flex flex-wrap items-center gap-3 border-b px-4 py-3">
        <CollapsedNavTrigger />
        <Skeleton className="h-7 w-36 max-w-full" />
        <Skeleton className="ml-auto h-8 w-56 max-w-full" />
      </div>
      <div className="space-y-3 px-4 py-3">
        {Array.from({ length: 5 }, (_, index) => (
          <div
            key={index}
            className="flex items-center gap-4 border-b py-3 last:border-0"
          >
            <Skeleton className="size-8 rounded-full" />
            <Skeleton className="h-4 w-36 min-w-0" />
            <Skeleton className="ml-auto h-4 w-40 min-w-0" />
          </div>
        ))}
      </div>
    </div>
  );
}

function InviteDialog({
  open,
  onOpenChange,
  canManage,
  onSubmit,
  isPending,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  canManage: boolean;
  onSubmit: (email: string, role: MemberRole) => void;
  isPending: boolean;
}) {
  const { t } = useT("teams");
  const [email, setEmail] = useState("");
  const [role, setRole] = useState<MemberRole>("member");
  const valid = /.+@.+\..+/.test(email.trim());

  useEffect(() => {
    if (!open) {
      setEmail("");
      setRole("member");
    }
  }, [open]);

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>{t(($) => $.directory.invite_title)}</DialogTitle>
          <DialogDescription>
            {t(($) => $.directory.invite_description)}
          </DialogDescription>
        </DialogHeader>
        <form
          onSubmit={(event) => {
            event.preventDefault();
            if (valid && canManage && !isPending) onSubmit(email.trim(), role);
          }}
        >
          <FieldGroup className="gap-4">
            <Field>
              <FieldLabel htmlFor="team-directory-invite-email">
                {t(($) => $.directory.email_label)}
              </FieldLabel>
              <InputGroup>
                <InputGroupInput
                  id="team-directory-invite-email"
                  type="email"
                  autoFocus
                  value={email}
                  placeholder={t(($) => $.directory.email_placeholder)}
                  autoComplete="off"
                  name="email"
                  spellCheck={false}
                  onChange={(event) => setEmail(event.target.value)}
                />
              </InputGroup>
            </Field>
            <Field>
              <FieldLabel htmlFor="team-directory-invite-role">
                {t(($) => $.directory.role_label)}
              </FieldLabel>
              <Select
                items={[
                  { value: "member", label: roleLabel("member", t) },
                  { value: "admin", label: roleLabel("admin", t) },
                ]}
                value={role}
                onValueChange={(value) => value && setRole(value as MemberRole)}
              >
                <SelectTrigger
                  id="team-directory-invite-role"
                  className="w-full"
                >
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectGroup>
                    {(["member", "admin"] as const).map((option) => (
                      <SelectItem key={option} value={option}>
                        {roleLabel(option, t)}
                      </SelectItem>
                    ))}
                  </SelectGroup>
                </SelectContent>
              </Select>
            </Field>
          </FieldGroup>
          <DialogFooter className="mt-5">
            <Button
              type="button"
              variant="outline"
              onClick={() => onOpenChange(false)}
            >
              {t(($) => $.directory.cancel)}
            </Button>
            <Button type="submit" disabled={!valid || !canManage || isPending}>
              <MailPlusIcon aria-hidden="true" />
              {isPending
                ? t(($) => $.directory.inviting)
                : t(($) => $.directory.send_invite)}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

function ViewSettingsMenu({
  density,
  onDensityChange,
  toggleColumns,
  columnVisibility,
  onToggleColumn,
  labels,
}: {
  density: TableDensity;
  onDensityChange: (value: TableDensity) => void;
  toggleColumns: readonly { id: string; label: string }[];
  columnVisibility: ColumnVisibilityState;
  onToggleColumn: (id: string) => void;
  labels: {
    table: string;
    density: string;
    columns: string;
    comfortable: string;
    compact: string;
  };
}) {
  const densityOptions = [
    { value: "comfortable", label: labels.comfortable },
    { value: "compact", label: labels.compact },
  ];

  return (
    <Popover>
      <PopoverTrigger
        render={
          <Button type="button" variant="outline">
            <Settings2Icon aria-hidden="true" />
            {labels.table}
          </Button>
        }
      />
      <PopoverContent align="end" className="w-[320px] p-0">
        <FieldGroup className="gap-3 px-3.5 py-3">
          <div className="space-y-2">
            <div className="text-caption font-medium text-muted-foreground">
              {labels.table}
            </div>
            <Field
              orientation="horizontal"
              className="min-h-9 items-center justify-between gap-3"
            >
              <FieldLabel className="text-body font-normal">
                {labels.density}
              </FieldLabel>
              <Select
                value={density}
                onValueChange={(value) =>
                  onDensityChange(value as TableDensity)
                }
                items={densityOptions}
              >
                <SelectTrigger size="sm" className="w-[140px] shrink-0">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent align="end">
                  <SelectGroup>
                    {densityOptions.map((option) => (
                      <SelectItem key={option.value} value={option.value}>
                        {option.label}
                      </SelectItem>
                    ))}
                  </SelectGroup>
                </SelectContent>
              </Select>
            </Field>
          </div>
          <Separator className="-mx-3.5" />
          <div className="space-y-2.5">
            <div className="text-caption font-medium text-muted-foreground">
              {labels.columns}
            </div>
            <div className="flex flex-wrap gap-1.5">
              {toggleColumns.map((column) => {
                const active = columnVisibility[column.id] !== false;
                return (
                  <Button
                    key={column.id}
                    type="button"
                    size="xs"
                    variant={active ? "secondary" : "outline"}
                    className="rounded-full"
                    onClick={() => onToggleColumn(column.id)}
                  >
                    {active ? (
                      <CheckIcon className="size-3.5" aria-hidden="true" />
                    ) : null}
                    {column.label}
                  </Button>
                );
              })}
            </div>
          </div>
        </FieldGroup>
      </PopoverContent>
    </Popover>
  );
}

function DirectoryGrid<TData extends object>({
  columns,
  data,
  recordCount,
  density,
  columnVisibility,
  onColumnVisibilityChange,
  sorting,
  onSortingChange,
  emptyMessage,
  pageLabel,
  paginationLabels,
}: {
  columns: ColumnDef<DataGridFeatures, TData>[];
  data: TData[];
  recordCount: number;
  density: TableDensity;
  columnVisibility: ColumnVisibilityState;
  onColumnVisibilityChange: (value: ColumnVisibilityState) => void;
  sorting: SortingState;
  onSortingChange: (value: SortingState) => void;
  emptyMessage: string;
  pageLabel: string;
  paginationLabels: {
    rowsPerPage: string;
    previous: string;
    next: string;
  };
}) {
  const table = useTable({
    features: dataGridFeatures,
    data,
    columns,
    getRowId: (row, index) => (row as { id?: string }).id ?? String(index),
    state: { columnVisibility, sorting },
    onColumnVisibilityChange: (updater) =>
      onColumnVisibilityChange(
        typeof updater === "function" ? updater(columnVisibility) : updater,
      ),
    onSortingChange: (updater) =>
      onSortingChange(
        typeof updater === "function" ? updater(sorting) : updater,
      ),
    initialState: { pagination: { pageIndex: 0, pageSize: PAGE_SIZES[0] } },
  });

  return (
    <DataGrid
      table={table}
      recordCount={recordCount}
      emptyMessage={emptyMessage}
      tableLayout={{ dense: density === "compact", width: "fixed" }}
      tableClassNames={{ bodyRow: "group/member-row" }}
    >
      <DataGridScrollArea>
        <DataGridTable />
      </DataGridScrollArea>
      <Separator />
      <div className="px-4 py-3">
        {recordCount > 0 ? (
          <DataGridPagination
            sizes={[...PAGE_SIZES]}
            info={"{from} - {to} of {count} " + pageLabel}
            rowsPerPageLabel={paginationLabels.rowsPerPage}
            previousPageLabel={paginationLabels.previous}
            nextPageLabel={paginationLabels.next}
            className="py-0"
          />
        ) : (
          <p className="text-body text-muted-foreground">0 {pageLabel}</p>
        )}
      </div>
    </DataGrid>
  );
}

export function TeamDirectoryPage() {
  const { t } = useT("teams");
  const { t: settingsT } = useT("settings");
  const { t: billingT } = useT("billing");
  const locale = useLocale();
  const workspace = useCurrentWorkspace();
  const workspacePaths = useWorkspacePaths();
  const wsId = workspace?.id ?? "";
  const currentUser = useAuthStore((state) => state.user);
  const queryClient = useQueryClient();
  const [activeTab, setActiveTab] = useState<DirectoryTab>("members");
  const [searchQuery, setSearchQuery] = useState("");
  const [density, setDensity] = useState<TableDensity>("comfortable");
  const [inviteOpen, setInviteOpen] = useState(false);
  const [inviteRecoveryPending, setInviteRecoveryPending] = useState(false);
  const [memberVisibility, setMemberVisibility] =
    useState<ColumnVisibilityState>({});
  const [invitationVisibility, setInvitationVisibility] =
    useState<ColumnVisibilityState>({});
  const [memberSorting, setMemberSorting] = useState<SortingState>([
    { id: "fullName", desc: false },
  ]);
  const [invitationSorting, setInvitationSorting] = useState<SortingState>([
    { id: "sentAt", desc: true },
  ]);
  const [pendingMember, setPendingMember] = useState<DirectoryMember | null>(
    null,
  );
  const [pendingInvitation, setPendingInvitation] =
    useState<DirectoryInvitation | null>(null);
  const [inviteSeatPurchase, setInviteSeatPurchase] =
    useState<DirectoryInviteSeatPurchase | null>(null);
  const dispatchedInvitePurchaseKey = useRef<string | null>(null);

  const previewSeatPurchase = usePreviewWorkspaceSeatPurchase();
  const purchaseSeats = usePurchaseWorkspaceSeats(wsId);

  const { data: members = [], isLoading: membersLoading } = useQuery({
    ...memberListOptions(wsId),
    enabled: !!wsId,
  });
  const { data: invitations = [], isLoading: invitationsLoading } = useQuery({
    ...invitationListOptions(wsId),
    enabled: !!wsId,
  });
  const seatPurchaseSummary = useQuery({
    ...workspaceSubscriptionSummaryOptions(wsId),
    enabled:
      inviteSeatPurchase?.phase === "waiting" &&
      inviteSeatPurchase.workspaceId === wsId,
    staleTime: 0,
    refetchInterval: inviteSeatPurchase?.phase === "waiting" ? 2_000 : false,
  });

  const currentMember = members.find(
    (member) => member.user_id === currentUser?.id,
  );
  const canManage =
    currentMember?.role === "owner" || currentMember?.role === "admin";
  const canManageOwners = currentMember?.role === "owner";
  const ownerCount = members.filter((member) => member.role === "owner").length;
  const normalizedQuery = searchQuery.trim().toLowerCase();
  const memberRows = useMemo(
    () => members.map((member) => toDirectoryMember(member, locale)),
    [locale, members],
  );
  const invitationRows = useMemo(
    () =>
      invitations.map((invitation) =>
        toDirectoryInvitation(invitation, locale),
      ),
    [invitations, locale],
  );
  const filteredMembers = useMemo(
    () =>
      memberRows.filter((member) =>
        [
          member.fullName,
          member.displayName,
          member.email,
          member.role,
          member.joinedAt,
        ]
          .join(" ")
          .toLowerCase()
          .includes(normalizedQuery),
      ),
    [memberRows, normalizedQuery],
  );
  const filteredInvitations = useMemo(
    () =>
      invitationRows.filter((invitation) =>
        [
          invitation.email,
          invitation.handle,
          invitation.role,
          invitation.invitedBy,
          invitation.sentAt,
          invitation.status,
        ]
          .join(" ")
          .toLowerCase()
          .includes(normalizedQuery),
      ),
    [invitationRows, normalizedQuery],
  );

  const invalidateDirectory = useCallback(async () => {
    await Promise.all([
      queryClient.invalidateQueries({ queryKey: workspaceKeys.members(wsId) }),
      queryClient.invalidateQueries({
        queryKey: workspaceKeys.invitations(wsId),
      }),
    ]);
  }, [queryClient, wsId]);

  const inviteMutation = useMutation({
    mutationFn: async ({
      email,
      role,
    }: {
      email: string;
      role: MemberRole;
    }) => {
      if (!workspace) throw new Error("Workspace is not ready");
      return api.createMember(workspace.id, { email, role });
    },
    onSuccess: async () => {
      await invalidateDirectory();
      setInviteOpen(false);
      toast.success(t(($) => $.directory.invite_sent));
    },
  });

  const sendInvitation = useCallback(
    async (email: string, role: MemberRole) => {
      await inviteMutation.mutateAsync({ email, role });
    },
    [inviteMutation],
  );

  const handleInvite = useCallback(
    async (email: string, role: MemberRole) => {
      setInviteRecoveryPending(true);
      try {
        await sendInvitation(email, role);
      } catch (error) {
        const code = errorCode(error);
        switch (classifyDirectoryInviteError(code)) {
          case "purchase": {
            try {
              const preview = await previewSeatPurchase.mutateAsync({
                additionalSeats: 1,
              });
              if (!isSingleSeatInvitePreview(preview)) {
                toast.error(
                  billingT(($) => $.workspace.seat_purchase.preview_unreadable),
                );
                return;
              }
              setInviteOpen(false);
              setInviteSeatPurchase({
                workspaceId: workspace?.id ?? wsId,
                email,
                role,
                preview,
                idempotencyKey: createSeatPurchaseKey(workspace?.id ?? wsId),
                phase: "review",
              });
              dispatchedInvitePurchaseKey.current = null;
            } catch (previewError) {
              toast.error(
                errorCode(previewError) === "seat_purchase_in_progress"
                  ? billingT(($) => $.workspace.seat_purchase.in_progress)
                  : billingT(($) => $.workspace.seat_purchase.preview_failed),
              );
            }
            return;
          }
          case "overcommitted": {
            try {
              const summary = await queryClient.fetchQuery({
                ...workspaceSubscriptionSummaryOptions(wsId),
                staleTime: 0,
              });
              const capacity = summary?.seatCapacity;
              toast.error(
                billingT(($) => $.workspace.seats.members_over_capacity_title),
                capacity
                  ? {
                      description: billingT(
                        ($) =>
                          $.workspace.seats.occupancy_over_capacity_description,
                        {
                          occupied: capacity.used + capacity.reserved,
                          purchased: capacity.purchased,
                          members: capacity.used,
                          reserved: capacity.reserved,
                        },
                      ),
                    }
                  : undefined,
              );
            } catch {
              toast.error(
                billingT(($) => $.workspace.seats.members_over_capacity_title),
              );
            }
            return;
          }
          case "unavailable":
            toast.error(
              settingsT(($) => $.members.toast_seat_capacity_unavailable),
            );
            return;
          case "rate_limited":
            toast.error(
              settingsT(($) => $.members.toast_seat_capacity_rate_limited),
            );
            return;
          default:
            toast.error(
              error instanceof Error
                ? error.message
                : t(($) => $.directory.invite_failed),
            );
        }
      } finally {
        setInviteRecoveryPending(false);
      }
    },
    [
      billingT,
      previewSeatPurchase,
      queryClient,
      settingsT,
      sendInvitation,
      t,
      workspace,
      wsId,
    ],
  );

  const roleMutation = useMutation({
    mutationFn: async ({
      memberId,
      role,
    }: {
      memberId: string;
      role: MemberRole;
    }) => {
      if (!workspace) throw new Error("Workspace is not ready");
      return api.updateMember(workspace.id, memberId, { role });
    },
    onSuccess: async () => {
      await invalidateDirectory();
      toast.success(t(($) => $.directory.role_updated));
    },
    onError: () => toast.error(t(($) => $.directory.action_failed)),
  });

  const removeMutation = useMutation({
    mutationFn: async (member: DirectoryMember) => {
      if (!workspace) throw new Error("Workspace is not ready");
      return api.deleteMember(workspace.id, member.id);
    },
    onSuccess: async () => {
      await invalidateDirectory();
      toast.success(t(($) => $.directory.remove_success));
    },
    onError: () => toast.error(t(($) => $.directory.action_failed)),
  });

  const resendMutation = useMutation({
    mutationFn: async (invitation: DirectoryInvitation) => {
      if (!workspace) throw new Error("Workspace is not ready");
      return api.resendInvitation(workspace.id, invitation.id);
    },
    onSuccess: () => toast.success(t(($) => $.directory.resend_success)),
    onError: () => toast.error(t(($) => $.directory.action_failed)),
  });

  const revokeMutation = useMutation({
    mutationFn: async (invitation: DirectoryInvitation) => {
      if (!workspace) throw new Error("Workspace is not ready");
      return api.revokeInvitation(workspace.id, invitation.id);
    },
    onSuccess: async () => {
      await invalidateDirectory();
      toast.success(t(($) => $.directory.revoke_success));
    },
    onError: () => toast.error(t(($) => $.directory.action_failed)),
  });

  const handlePurchaseSeatAndInvite = useCallback(async () => {
    if (
      !inviteSeatPurchase ||
      (inviteSeatPurchase.phase !== "review" &&
        !(inviteSeatPurchase.phase === "error" &&
          inviteSeatPurchase.retryable))
    ) {
      return;
    }
    const current = inviteSeatPurchase;
    if (!workspace || current.workspaceId !== workspace.id) {
      setInviteSeatPurchase(null);
      return;
    }

    dispatchedInvitePurchaseKey.current = null;
    setInviteSeatPurchase({
      ...current,
      phase: "purchasing",
      error: undefined,
      retryable: false,
    });
    const request: PurchaseWorkspaceSeatsRequest = {
      additionalSeats: current.preview.additionalSeats,
      expectedCurrentSeats: current.preview.currentSeats,
      expectedPurchaseVersion: current.preview.purchaseVersion,
      acceptedProrationAmount: current.preview.prorationAmount,
      currency: current.preview.currency,
      idempotencyKey: current.idempotencyKey,
    };
    try {
      const response = await purchaseSeats.mutateAsync(request);
      if (!seatPurchaseMatchesPreview(response, current.preview)) {
        setInviteSeatPurchase({
          ...current,
          phase: "error",
          error: billingT(
            ($) => $.workspace.seat_purchase.purchase_unreadable,
          ),
          retryable: true,
        });
        return;
      }
      const submittedAt = Date.now();
      setInviteSeatPurchase({
        ...current,
        phase: "waiting",
        submittedAt,
        error: undefined,
        retryable: false,
      });
      await queryClient.invalidateQueries({
        queryKey: workspaceSubscriptionSummaryOptions(wsId).queryKey,
      });
    } catch (error) {
      const code = errorCode(error);
      const message =
        code === "seat_purchase_payment_failed"
          ? billingT(($) => $.workspace.seat_purchase.payment_failed)
          : code === "seat_purchase_in_progress"
            ? billingT(($) => $.workspace.seat_purchase.in_progress)
            : code === "seat_quote_changed" || code === "seat_capacity_changed"
              ? billingT(($) => $.workspace.seat_purchase.quote_changed)
              : billingT(($) => $.workspace.seat_purchase.purchase_failed);
      setInviteSeatPurchase({
        ...current,
        phase: "error",
        error: message,
        retryable: seatPurchaseCanRetryWithSameQuote(code),
      });
    }
  }, [
    billingT,
    inviteSeatPurchase,
    purchaseSeats,
    queryClient,
    workspace,
    wsId,
  ]);

  useEffect(() => {
    setInviteSeatPurchase((current) =>
      current && current.workspaceId !== wsId ? null : current,
    );
  }, [wsId]);

  useEffect(() => {
    const purchase = inviteSeatPurchase;
    if (
      purchase?.phase !== "waiting" ||
      purchase.workspaceId !== wsId ||
      purchase.submittedAt == null ||
      dispatchedInvitePurchaseKey.current === purchase.idempotencyKey ||
      !purchasedSeatIsReadyForInvitation(
        seatPurchaseSummary.data,
        purchase.preview,
        purchase.submittedAt,
        seatPurchaseSummary.dataUpdatedAt,
      )
    ) {
      return;
    }

    dispatchedInvitePurchaseKey.current = purchase.idempotencyKey;
    setInviteSeatPurchase({ ...purchase, phase: "inviting" });
    void sendInvitation(purchase.email, purchase.role)
      .then(() => setInviteSeatPurchase(null))
      .catch((error) => {
        const code = errorCode(error);
        const kind = classifyDirectoryInviteError(code);
        let message: string;
        switch (kind) {
          case "purchase":
            message = settingsT(($) => $.members.seat_purchase_capacity_taken);
            break;
          case "unavailable":
            message = settingsT(
              ($) => $.members.toast_seat_capacity_unavailable,
            );
            break;
          case "rate_limited":
            message = settingsT(
              ($) => $.members.toast_seat_capacity_rate_limited,
            );
            break;
          case "overcommitted":
          case "unknown":
          default:
            message =
              error instanceof Error
                ? error.message
                : t(($) => $.directory.invite_failed);
            break;
        }
        setInviteSeatPurchase({
          ...purchase,
          phase: "error",
          retryable: seatInvitationCanRetryAfterPurchase(code),
          error: message,
        });
      });
  }, [
    inviteSeatPurchase,
    seatPurchaseSummary.data,
    seatPurchaseSummary.dataUpdatedAt,
    sendInvitation,
    settingsT,
    t,
    wsId,
  ]);

  useEffect(() => {
    if (
      inviteSeatPurchase?.phase !== "waiting" ||
      inviteSeatPurchase.submittedAt == null
    ) {
      return;
    }
    const elapsed = Date.now() - inviteSeatPurchase.submittedAt;
    const timeout = window.setTimeout(() => {
      setInviteSeatPurchase((current) =>
        current?.phase === "waiting"
          ? {
              ...current,
              phase: "error",
              error: settingsT(($) => $.members.seat_purchase_timeout),
              retryable: false,
            }
          : current,
      );
    }, Math.max(0, SEAT_PURCHASE_CONFIRM_TIMEOUT_MS - elapsed));
    return () => window.clearTimeout(timeout);
  }, [inviteSeatPurchase?.phase, inviteSeatPurchase?.submittedAt, settingsT]);

  const memberLabels = useMemo(
    () => ({
      member: t(($) => $.directory.member_column),
      email: t(($) => $.directory.email_column),
      role: t(($) => $.directory.role_column),
      status: t(($) => $.directory.status_column),
      joined: t(($) => $.directory.joined_column),
      active: t(($) => $.directory.active_status),
      owner: t(($) => $.directory.owner_role),
      admin: t(($) => $.directory.admin_role),
      memberRole: t(($) => $.directory.member_role),
      actions: t(($) => $.directory.actions),
      remove: t(($) => $.directory.remove_member),
      roleMenu: t(($) => $.directory.role_column),
    }),
    [t],
  );
  const invitationLabels = useMemo(
    () => ({
      invitee: t(($) => $.directory.email_column),
      role: t(($) => $.directory.role_column),
      admin: t(($) => $.directory.admin_role),
      memberRole: t(($) => $.directory.member_role),
      invitedBy: t(($) => $.directory.invited_by_column),
      sent: t(($) => $.directory.sent_column),
      status: t(($) => $.directory.status_column),
      actions: t(($) => $.directory.actions),
      resend: t(($) => $.directory.resend),
      revoke: t(($) => $.directory.revoke_invitation),
      statusValues: {
        pending: t(($) => $.directory.pending_status),
        accepted: t(($) => $.directory.accepted_status),
        declined: t(($) => $.directory.declined_status),
        expired: t(($) => $.directory.expired_status),
      },
    }),
    [t],
  );

  const handleMemberAction = useCallback(
    (action: MemberRowAction, member: DirectoryMember) => {
      if (action.type === "role") {
        if (action.role !== member.role) {
          roleMutation.mutate({ memberId: member.id, role: action.role });
        }
        return;
      }
      setPendingMember(member);
    },
    [roleMutation],
  );
  const handleInvitationAction = useCallback(
    (action: InvitationRowAction, invitation: DirectoryInvitation) => {
      if (action === "resend") {
        resendMutation.mutate(invitation);
        return;
      }
      setPendingInvitation(invitation);
    },
    [resendMutation],
  );

  const memberColumns = useMemo(
    () =>
      createMemberGridColumns({
        canManage: !!canManage && !roleMutation.isPending,
        canManageOwners,
        currentUserId: currentUser?.id,
        ownerCount,
        labels: memberLabels,
        onAction: handleMemberAction,
      }),
    [
      canManage,
      canManageOwners,
      currentUser?.id,
      handleMemberAction,
      memberLabels,
      ownerCount,
      roleMutation.isPending,
    ],
  );
  const invitationColumns = useMemo(
    () =>
      createInvitationGridColumns({
        canManage:
          !!canManage && !resendMutation.isPending && !revokeMutation.isPending,
        labels: invitationLabels,
        onAction: handleInvitationAction,
      }),
    [
      canManage,
      handleInvitationAction,
      invitationLabels,
      resendMutation.isPending,
      revokeMutation.isPending,
    ],
  );

  const memberToggleColumns = useMemo(
    () =>
      MEMBER_TOGGLE_COLUMNS.map(({ id }) => ({
        id,
        label:
          id === "role"
            ? memberLabels.role
            : id === "status"
              ? memberLabels.status
              : memberLabels.joined,
      })),
    [memberLabels],
  );
  const invitationToggleColumns = useMemo(
    () =>
      INVITATION_TOGGLE_COLUMNS.map(({ id }) => ({
        id,
        label:
          id === "role"
            ? invitationLabels.role
            : id === "invitedBy"
              ? invitationLabels.invitedBy
              : id === "sentAt"
                ? invitationLabels.sent
                : invitationLabels.status,
      })),
    [invitationLabels],
  );
  const activeVisibility =
    activeTab === "members" ? memberVisibility : invitationVisibility;
  const toggleColumns =
    activeTab === "members" ? memberToggleColumns : invitationToggleColumns;

  const handleToggleColumn = useCallback(
    (id: string) => {
      const setter =
        activeTab === "members" ? setMemberVisibility : setInvitationVisibility;
      setter((current) => ({
        ...current,
        [id]: current[id] === false,
      }));
    },
    [activeTab],
  );

  if (!workspace) return null;
  const loading = membersLoading || invitationsLoading;

  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col overflow-y-auto">
      {loading ? (
        <DirectorySkeleton />
      ) : (
        <Tabs
          value={activeTab}
          onValueChange={(value) => setActiveTab(value as DirectoryTab)}
          className="min-w-0 gap-0"
        >
          <div className="flex flex-wrap items-center gap-x-4 gap-y-2 border-b px-4 py-2">
            <div className="flex min-w-0 max-w-full flex-wrap items-center gap-3">
              <CollapsedNavTrigger />
              <TabsList
                variant="line"
                className="h-9 w-auto max-w-full justify-start gap-4 p-0!"
              >
                <TabsTrigger
                  value="members"
                  className="h-full! flex-none! gap-2 px-0 text-body"
                >
                  <span>{t(($) => $.directory.members_tab)}</span>
                  <TabCount
                    active={activeTab === "members"}
                    count={members.length}
                  />
                </TabsTrigger>
                <TabsTrigger
                  value="invitations"
                  className="h-full! flex-none! gap-2 px-0 text-body"
                >
                  <span>{t(($) => $.directory.invitations_tab)}</span>
                  <TabCount
                    active={activeTab === "invitations"}
                    count={invitations.length}
                  />
                </TabsTrigger>
              </TabsList>
              <AppLink
                href={workspacePaths.agentTeams()}
                className="shrink-0 rounded-md px-2 py-1 text-caption text-muted-foreground transition-colors hover:bg-muted hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
              >
                {t(($) => $.directory.agent_teams_link)}
              </AppLink>
            </div>
            <div className="ml-auto flex min-w-0 max-w-full flex-wrap items-center justify-end gap-2">
              <InputGroup className="w-full min-w-0 sm:w-56">
                <InputGroupAddon align="inline-start">
                  <SearchIcon aria-hidden="true" />
                </InputGroupAddon>
                <InputGroupInput
                  name="team-directory-search"
                  autoComplete="off"
                  value={searchQuery}
                  onChange={(event) => setSearchQuery(event.target.value)}
                  placeholder={
                    activeTab === "members"
                      ? t(($) => $.directory.search_members)
                      : t(($) => $.directory.search_invitations)
                  }
                  aria-label={
                    activeTab === "members"
                      ? t(($) => $.directory.search_members)
                      : t(($) => $.directory.search_invitations)
                  }
                />
                {searchQuery.length > 0 ? (
                  <InputGroupAddon align="inline-end">
                    <InputGroupButton
                      size="icon-xs"
                      aria-label={t(($) => $.directory.clear_search)}
                      onClick={() => setSearchQuery("")}
                    >
                      <XIcon aria-hidden="true" />
                    </InputGroupButton>
                  </InputGroupAddon>
                ) : null}
              </InputGroup>
              <ViewSettingsMenu
                density={density}
                onDensityChange={setDensity}
                toggleColumns={toggleColumns}
                columnVisibility={activeVisibility}
                onToggleColumn={handleToggleColumn}
                labels={{
                  table: t(($) => $.directory.view_settings),
                  density: t(($) => $.directory.density),
                  columns: t(($) => $.directory.columns),
                  comfortable: t(($) => $.directory.density_comfortable),
                  compact: t(($) => $.directory.density_compact),
                }}
              />
              <Button
                type="button"
                disabled={!canManage}
                onClick={() => setInviteOpen(true)}
              >
                <MailPlusIcon aria-hidden="true" />
                {t(($) => $.directory.invite_people)}
              </Button>
            </div>
          </div>
          {activeTab === "members" ? (
            <DirectoryGrid
              columns={memberColumns}
              data={filteredMembers}
              recordCount={filteredMembers.length}
              density={density}
              columnVisibility={memberVisibility}
              onColumnVisibilityChange={setMemberVisibility}
              sorting={memberSorting}
              onSortingChange={setMemberSorting}
              emptyMessage={t(($) => $.directory.no_members)}
              pageLabel={t(($) => $.directory.range_members)}
              paginationLabels={{
                rowsPerPage: t(($) => $.directory.rows_per_page),
                previous: t(($) => $.directory.previous_page),
                next: t(($) => $.directory.next_page),
              }}
            />
          ) : (
            <DirectoryGrid
              columns={invitationColumns}
              data={filteredInvitations}
              recordCount={filteredInvitations.length}
              density={density}
              columnVisibility={invitationVisibility}
              onColumnVisibilityChange={setInvitationVisibility}
              sorting={invitationSorting}
              onSortingChange={setInvitationSorting}
              emptyMessage={t(($) => $.directory.no_invitations)}
              pageLabel={t(($) => $.directory.range_invitations)}
              paginationLabels={{
                rowsPerPage: t(($) => $.directory.rows_per_page),
                previous: t(($) => $.directory.previous_page),
                next: t(($) => $.directory.next_page),
              }}
            />
          )}
        </Tabs>
      )}

      <InviteDialog
        open={inviteOpen}
        onOpenChange={setInviteOpen}
        canManage={!!canManage}
        isPending={inviteMutation.isPending || inviteRecoveryPending}
        onSubmit={(email, role) => void handleInvite(email, role)}
      />

      <AlertDialog
        open={inviteSeatPurchase !== null}
        onOpenChange={(open) => {
          if (
            !open &&
            (inviteSeatPurchase?.phase === "review" ||
              inviteSeatPurchase?.phase === "error")
          ) {
            setInviteSeatPurchase(null);
          }
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>
              {settingsT(($) => $.members.seat_purchase_title)}
            </AlertDialogTitle>
            <AlertDialogDescription>
              {inviteSeatPurchase?.phase === "review"
                ? settingsT(($) => $.members.seat_purchase_description, {
                    email: inviteSeatPurchase.email,
                  })
                : inviteSeatPurchase?.phase === "error"
                  ? inviteSeatPurchase.error
                  : settingsT(($) => $.members.seat_purchase_waiting)}
            </AlertDialogDescription>
          </AlertDialogHeader>

          {inviteSeatPurchase?.phase === "review" && (
            <div className="divide-y rounded-lg border text-body">
              <div className="flex items-center justify-between gap-4 px-4 py-3">
                <span className="text-muted-foreground">
                  {billingT(($) => $.workspace.seat_purchase.seats_after)}
                </span>
                <span className="font-medium">
                  {billingT(($) => $.workspace.seats.seat_count, {
                    count: inviteSeatPurchase.preview.resultingSeats,
                  })}
                </span>
              </div>
              <div className="flex items-center justify-between gap-4 px-4 py-3">
                <span className="text-muted-foreground">
                  {billingT(($) => $.workspace.seat_purchase.charge_today)}
                </span>
                <span className="font-medium">
                  {formatStripeMinorAmount(
                    inviteSeatPurchase.preview.prorationAmount,
                    inviteSeatPurchase.preview.currency,
                    locale,
                  ) ?? "—"}
                </span>
              </div>
              <div className="flex items-center justify-between gap-4 px-4 py-3">
                <span className="text-muted-foreground">
                  {billingT(($) => $.workspace.seat_purchase.next_invoice)}
                </span>
                <span className="font-medium">
                  {formatStripeMinorAmount(
                    inviteSeatPurchase.preview.nextInvoiceAmount,
                    inviteSeatPurchase.preview.currency,
                    locale,
                  ) ?? "—"}
                </span>
              </div>
              <p className="px-4 py-3 text-caption text-muted-foreground">
                {billingT(($) => $.workspace.seat_purchase.tax_notice)}
              </p>
            </div>
          )}

          {(inviteSeatPurchase?.phase === "purchasing" ||
            inviteSeatPurchase?.phase === "waiting" ||
            inviteSeatPurchase?.phase === "inviting") && (
            <div className="flex items-center gap-2 text-body text-muted-foreground">
              <Loader2Icon className="size-4 animate-spin" aria-hidden="true" />
              {inviteSeatPurchase.phase === "inviting"
                ? settingsT(($) => $.members.inviting)
                : settingsT(($) => $.members.seat_purchase_waiting)}
            </div>
          )}

          <AlertDialogFooter>
            {(inviteSeatPurchase?.phase === "review" ||
              inviteSeatPurchase?.phase === "error") && (
              <AlertDialogCancel>
                {settingsT(($) => $.members.confirm_cancel)}
              </AlertDialogCancel>
            )}
            {inviteSeatPurchase?.phase === "review" && (
              <AlertDialogAction
                onClick={(event) => {
                  event.preventDefault();
                  void handlePurchaseSeatAndInvite();
                }}
              >
                {settingsT(($) => $.members.purchase_seat_and_invite)}
              </AlertDialogAction>
            )}
            {inviteSeatPurchase?.phase === "error" &&
              inviteSeatPurchase.retryable && (
                <AlertDialogAction
                  onClick={(event) => {
                    event.preventDefault();
                    void handlePurchaseSeatAndInvite();
                  }}
                >
                  {settingsT(($) => $.members.retry_seat_purchase)}
                </AlertDialogAction>
              )}
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

      <AlertDialog
        open={pendingMember !== null}
        onOpenChange={(open) => {
          if (!open) setPendingMember(null);
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>
              {t(($) => $.directory.remove_confirm, {
                name: pendingMember?.fullName ?? "",
              })}
            </AlertDialogTitle>
            <AlertDialogDescription>
              {t(($) => $.directory.remove_description, {
                name: pendingMember?.fullName ?? "",
              })}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>
              {t(($) => $.directory.cancel)}
            </AlertDialogCancel>
            <AlertDialogAction
              disabled={removeMutation.isPending}
              className="bg-destructive text-white hover:bg-destructive/90"
              onClick={() => {
                if (pendingMember) removeMutation.mutate(pendingMember);
                setPendingMember(null);
              }}
            >
              {t(($) => $.directory.remove_member)}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

      <AlertDialog
        open={pendingInvitation !== null}
        onOpenChange={(open) => {
          if (!open) setPendingInvitation(null);
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>
              {t(($) => $.directory.revoke_confirm)}
            </AlertDialogTitle>
            <AlertDialogDescription>
              {t(($) => $.directory.revoke_description, {
                email: pendingInvitation?.email ?? "",
              })}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>
              {t(($) => $.directory.cancel)}
            </AlertDialogCancel>
            <AlertDialogAction
              disabled={revokeMutation.isPending}
              className="bg-destructive text-white hover:bg-destructive/90"
              onClick={() => {
                if (pendingInvitation) revokeMutation.mutate(pendingInvitation);
                setPendingInvitation(null);
              }}
            >
              {t(($) => $.directory.revoke_invitation)}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}
