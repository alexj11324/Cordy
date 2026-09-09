"use client";

import type { MemberRole } from "@orvilo/core/types";
import { DataGridColumnHeader } from "@orvilo/ui/components/reui/data-grid/data-grid-column-header";
import type { DataGridFeatures } from "@orvilo/ui/components/reui/data-grid/data-grid";
import { Badge, type BadgeProps } from "@orvilo/ui/components/reui/badge";
import {
  Avatar,
  AvatarFallback,
  AvatarImage,
} from "@orvilo/ui/components/ui/avatar";
import { Button } from "@orvilo/ui/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
} from "@orvilo/ui/components/ui/dropdown-menu";
import type { ColumnDef } from "@tanstack/react-table";
import {
  CheckIcon,
  MoreHorizontalIcon,
  RotateCwIcon,
  ShieldIcon,
  Trash2Icon,
  XIcon,
} from "lucide-react";
import type {
  DirectoryInvitation,
  DirectoryMember,
} from "./team-directory-data";
import {
  memberActionAvailability,
  memberRoleOptions,
} from "./team-directory-permissions";

export type MemberRowAction =
  | { type: "role"; role: MemberRole }
  | { type: "remove" };

export type InvitationRowAction = "resend" | "revoke";

type MemberLabels = {
  member: string;
  email: string;
  role: string;
  status: string;
  joined: string;
  active: string;
  owner: string;
  admin: string;
  memberRole: string;
  actions: string;
  remove: string;
  roleMenu: string;
};

type InvitationLabels = {
  invitee: string;
  role: string;
  admin: string;
  memberRole: string;
  invitedBy: string;
  sent: string;
  status: string;
  actions: string;
  resend: string;
  revoke: string;
  statusValues: Record<DirectoryInvitation["status"], string>;
};

function MemberRowActions({
  member,
  canManage,
  canManageOwners,
  currentUserId,
  ownerCount,
  labels,
  onAction,
}: {
  member: DirectoryMember;
  canManage: boolean;
  canManageOwners: boolean;
  currentUserId: string | undefined;
  ownerCount: number;
  labels: MemberLabels;
  onAction: (action: MemberRowAction, member: DirectoryMember) => void;
}) {
  const { canEditRole, canRemove } = memberActionAvailability({
    canManage,
    canManageOwners,
    isSelf: member.userId === currentUserId,
    memberRole: member.role,
  });
  if (!canEditRole && !canRemove) {
    return null;
  }

  return (
    <DropdownMenu>
      <DropdownMenuTrigger
        render={
          <Button
            type="button"
            size="icon-sm"
            variant="ghost"
            aria-label={`${labels.actions}: ${member.fullName}`}
          />
        }
      >
        <MoreHorizontalIcon aria-hidden="true" />
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end">
        <DropdownMenuLabel>{labels.roleMenu}</DropdownMenuLabel>
        <DropdownMenuSub>
          <DropdownMenuSubTrigger>
            <ShieldIcon aria-hidden="true" />
            {member.role === "owner"
              ? labels.owner
              : member.role === "admin"
                ? labels.admin
                : labels.memberRole}
          </DropdownMenuSubTrigger>
          <DropdownMenuSubContent>
            {memberRoleOptions({
              memberRole: member.role,
              canManageOwners,
              ownerCount,
            }).map(({ role, disabled }) => (
              <DropdownMenuItem
                key={role}
                disabled={disabled || !canEditRole}
                onClick={() => {
                  if (!disabled && canEditRole) {
                    onAction({ type: "role", role }, member);
                  }
                }}
              >
                {member.role === role ? (
                  <CheckIcon aria-hidden="true" />
                ) : (
                  <span className="size-3.5" aria-hidden="true" />
                )}
                {role === "owner"
                  ? labels.owner
                  : role === "admin"
                    ? labels.admin
                    : labels.memberRole}
              </DropdownMenuItem>
            ))}
          </DropdownMenuSubContent>
        </DropdownMenuSub>
        {canRemove ? (
          <>
            <DropdownMenuSeparator />
            <DropdownMenuItem
              variant="destructive"
              onClick={() => onAction({ type: "remove" }, member)}
            >
              <Trash2Icon aria-hidden="true" />
              {labels.remove}
            </DropdownMenuItem>
          </>
        ) : null}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

function MemberCell({
  member,
  canManage,
  canManageOwners,
  currentUserId,
  ownerCount,
  labels,
  onAction,
}: {
  member: DirectoryMember;
  canManage: boolean;
  canManageOwners: boolean;
  currentUserId: string | undefined;
  ownerCount: number;
  labels: MemberLabels;
  onAction: (action: MemberRowAction, member: DirectoryMember) => void;
}) {
  return (
    <div className="flex min-w-0 items-center gap-2">
      <Avatar className="size-8 shrink-0">
        {member.avatarSrc ? (
          <AvatarImage src={member.avatarSrc} alt={member.fullName} />
        ) : null}
        <AvatarFallback>{member.initials}</AvatarFallback>
      </Avatar>
      <div className="min-w-0 flex-1">
        <div className="truncate text-sm font-medium">{member.fullName}</div>
        <div className="truncate text-sm text-muted-foreground">
          {member.displayName}
        </div>
      </div>
      <div className="pointer-events-none flex shrink-0 items-center opacity-0 transition-opacity group-focus-within/member-row:pointer-events-auto group-focus-within/member-row:opacity-100 group-hover/member-row:pointer-events-auto group-hover/member-row:opacity-100">
        <MemberRowActions
          member={member}
          canManage={canManage}
          canManageOwners={canManageOwners}
          currentUserId={currentUserId}
          ownerCount={ownerCount}
          labels={labels}
          onAction={onAction}
        />
      </div>
    </div>
  );
}

function ActiveStatusCell({ label }: { label: string }) {
  return (
    <div className="flex min-w-0 items-center gap-2">
      <span
        className="size-1.5 shrink-0 rounded-full bg-emerald-500"
        aria-hidden="true"
      />
      <span className="truncate text-sm">{label}</span>
    </div>
  );
}

export const MEMBER_TOGGLE_COLUMNS = [
  { id: "role", labelKey: "role" },
  { id: "status", labelKey: "status" },
  { id: "joinedAt", labelKey: "joined" },
] as const;

export function createMemberGridColumns({
  canManage,
  canManageOwners,
  currentUserId,
  ownerCount,
  labels,
  onAction,
}: {
  canManage: boolean;
  canManageOwners: boolean;
  currentUserId: string | undefined;
  ownerCount: number;
  labels: MemberLabels;
  onAction: (action: MemberRowAction, member: DirectoryMember) => void;
}): ColumnDef<DataGridFeatures, DirectoryMember>[] {
  return [
    {
      accessorKey: "fullName",
      id: "fullName",
      header: ({ column }) => (
        <DataGridColumnHeader column={column} visibility={true} />
      ),
      cell: ({ row }) => (
        <MemberCell
          member={row.original}
          canManage={canManage}
          canManageOwners={canManageOwners}
          currentUserId={currentUserId}
          ownerCount={ownerCount}
          labels={labels}
          onAction={onAction}
        />
      ),
      size: 260,
      enableSorting: true,
      enableHiding: false,
      enableResizing: false,
      meta: { headerTitle: labels.member },
    },
    {
      accessorKey: "email",
      id: "email",
      header: ({ column }) => (
        <DataGridColumnHeader column={column} visibility={true} />
      ),
      cell: ({ row }) => (
        <a
          href={`mailto:${row.original.email}`}
          className="block truncate text-sm transition-colors hover:text-primary hover:underline"
          title={row.original.email}
        >
          {row.original.email}
        </a>
      ),
      size: 220,
      enableSorting: true,
      enableHiding: false,
      enableResizing: false,
      meta: { headerTitle: labels.email },
    },
    {
      accessorKey: "role",
      id: "role",
      header: ({ column }) => (
        <DataGridColumnHeader column={column} visibility={true} />
      ),
      cell: ({ row }) => (
        <span className="block truncate text-sm">
          {row.original.role === "admin"
            ? labels.admin
            : row.original.role === "owner"
              ? labels.owner
              : labels.memberRole}
        </span>
      ),
      size: 120,
      enableSorting: true,
      enableHiding: true,
      enableResizing: false,
      meta: { headerTitle: labels.role },
    },
    {
      accessorKey: "status",
      id: "status",
      header: ({ column }) => (
        <DataGridColumnHeader column={column} visibility={true} />
      ),
      cell: () => <ActiveStatusCell label={labels.active} />,
      size: 150,
      enableSorting: true,
      enableHiding: true,
      enableResizing: false,
      meta: { headerTitle: labels.status },
    },
    {
      accessorKey: "joinedAt",
      id: "joinedAt",
      header: ({ column }) => (
        <DataGridColumnHeader column={column} visibility={true} />
      ),
      cell: ({ row }) => (
        <span className="block truncate text-sm text-muted-foreground tabular-nums">
          {row.original.joinedAt}
        </span>
      ),
      sortFn: (rowA, rowB) =>
        new Date(rowA.original.joinedAt).getTime() -
        new Date(rowB.original.joinedAt).getTime(),
      size: 140,
      enableSorting: true,
      enableHiding: true,
      enableResizing: false,
      meta: { headerTitle: labels.joined },
    },
  ];
}

const invitationStatusVariant: Record<
  DirectoryInvitation["status"],
  BadgeProps["variant"]
> = {
  pending: "warning-light",
  accepted: "success-light",
  declined: "secondary",
  expired: "secondary",
};

function InvitationStatusBadge({
  invitation,
  labels,
}: {
  invitation: DirectoryInvitation;
  labels: InvitationLabels;
}) {
  return (
    <Badge variant={invitationStatusVariant[invitation.status]}>
      {labels.statusValues[invitation.status]}
    </Badge>
  );
}

function InviteeCell({ invitation }: { invitation: DirectoryInvitation }) {
  return (
    <div className="flex min-w-0 flex-col gap-px">
      <span className="truncate text-sm font-medium">{invitation.email}</span>
      <span className="truncate text-xs text-muted-foreground">
        @{invitation.handle}
      </span>
    </div>
  );
}

function InvitationRowActions({
  invitation,
  canManage,
  labels,
  onAction,
}: {
  invitation: DirectoryInvitation;
  canManage: boolean;
  labels: InvitationLabels;
  onAction: (
    action: InvitationRowAction,
    invitation: DirectoryInvitation,
  ) => void;
}) {
  if (!canManage || invitation.status !== "pending") return null;

  return (
    <div className="pointer-events-none flex shrink-0 items-center justify-end gap-1 opacity-0 transition-opacity group-focus-within/member-row:pointer-events-auto group-focus-within/member-row:opacity-100 group-hover/member-row:pointer-events-auto group-hover/member-row:opacity-100">
      <Button
        type="button"
        size="sm"
        variant="outline"
        onClick={() => onAction("resend", invitation)}
      >
        <RotateCwIcon data-icon="inline-start" aria-hidden="true" />
        {labels.resend}
      </Button>
      <Button
        type="button"
        size="icon-sm"
        variant="ghost"
        className="hover:text-destructive"
        aria-label={`${labels.revoke}: ${invitation.email}`}
        onClick={() => onAction("revoke", invitation)}
      >
        <XIcon aria-hidden="true" />
      </Button>
    </div>
  );
}

export const INVITATION_TOGGLE_COLUMNS = [
  { id: "role", labelKey: "role" },
  { id: "invitedBy", labelKey: "invitedBy" },
  { id: "sentAt", labelKey: "sent" },
  { id: "status", labelKey: "status" },
] as const;

export function createInvitationGridColumns({
  canManage,
  labels,
  onAction,
}: {
  canManage: boolean;
  labels: InvitationLabels;
  onAction: (
    action: InvitationRowAction,
    invitation: DirectoryInvitation,
  ) => void;
}): ColumnDef<DataGridFeatures, DirectoryInvitation>[] {
  return [
    {
      accessorKey: "email",
      id: "email",
      header: ({ column }) => (
        <DataGridColumnHeader column={column} visibility={true} />
      ),
      cell: ({ row }) => <InviteeCell invitation={row.original} />,
      size: 260,
      enableSorting: true,
      enableHiding: false,
      enableResizing: false,
      meta: { headerTitle: labels.invitee },
    },
    {
      accessorKey: "role",
      id: "role",
      header: ({ column }) => (
        <DataGridColumnHeader column={column} visibility={true} />
      ),
      cell: ({ row }) => (
        <span className="block truncate text-sm">
          {row.original.role === "admin" ? labels.admin : labels.memberRole}
        </span>
      ),
      size: 140,
      enableSorting: true,
      enableHiding: true,
      enableResizing: false,
      meta: { headerTitle: labels.role },
    },
    {
      accessorKey: "invitedBy",
      id: "invitedBy",
      header: ({ column }) => (
        <DataGridColumnHeader column={column} visibility={true} />
      ),
      cell: ({ row }) => (
        <span className="block truncate text-sm text-muted-foreground">
          {row.original.invitedBy}
        </span>
      ),
      size: 180,
      enableSorting: true,
      enableHiding: true,
      enableResizing: false,
      meta: { headerTitle: labels.invitedBy },
    },
    {
      accessorKey: "sentAt",
      id: "sentAt",
      header: ({ column }) => (
        <DataGridColumnHeader column={column} visibility={true} />
      ),
      cell: ({ row }) => (
        <span className="block truncate text-sm text-muted-foreground tabular-nums">
          {row.original.sentAt}
        </span>
      ),
      sortFn: (rowA, rowB) =>
        new Date(rowA.original.sentAt).getTime() -
        new Date(rowB.original.sentAt).getTime(),
      size: 140,
      enableSorting: true,
      enableHiding: true,
      enableResizing: false,
      meta: { headerTitle: labels.sent },
    },
    {
      accessorKey: "status",
      id: "status",
      header: ({ column }) => (
        <DataGridColumnHeader column={column} visibility={true} />
      ),
      cell: ({ row }) => (
        <InvitationStatusBadge invitation={row.original} labels={labels} />
      ),
      size: 130,
      enableSorting: true,
      enableHiding: true,
      enableResizing: false,
      meta: { headerTitle: labels.status },
    },
    {
      id: "actions",
      header: "",
      cell: ({ row }) => (
        <InvitationRowActions
          invitation={row.original}
          canManage={canManage}
          labels={labels}
          onAction={onAction}
        />
      ),
      size: 170,
      enableSorting: false,
      enableHiding: false,
      enableResizing: false,
      meta: {
        headerClassName: "pe-4!",
        cellClassName: "pe-4!",
      },
    },
  ];
}

export type { InvitationLabels, MemberLabels };
