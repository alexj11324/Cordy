"use client";

import { useEffect, useMemo, useState } from "react";
import {
  Check,
  ChevronLeft,
  ChevronRight,
  Clock3,
  Mail,
  MailPlus,
  MoreHorizontal,
  Plus,
  Search,
  Settings2,
  Shield,
  Trash2,
  X,
} from "lucide-react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { api, errorCode } from "@orvilo/core/api";
import { useAuthStore } from "@orvilo/core/auth";
import { useCurrentWorkspace } from "@orvilo/core/paths";
import {
  invitationListOptions,
  memberListOptions,
  workspaceKeys,
} from "@orvilo/core/workspace/queries";
import type { Invitation, MemberRole, MemberWithUser } from "@orvilo/core/types";
import { Avatar, AvatarFallback, AvatarImage } from "@orvilo/ui/components/ui/avatar";
import { Badge } from "@orvilo/ui/components/ui/badge";
import { Button } from "@orvilo/ui/components/ui/button";
import {
  Card,
  CardAction,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@orvilo/ui/components/ui/card";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@orvilo/ui/components/ui/dialog";
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
import { Input } from "@orvilo/ui/components/ui/input";
import { Popover, PopoverContent, PopoverTrigger } from "@orvilo/ui/components/ui/popover";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@orvilo/ui/components/ui/select";
import { Separator } from "@orvilo/ui/components/ui/separator";
import { Skeleton } from "@orvilo/ui/components/ui/skeleton";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@orvilo/ui/components/ui/table";
import { Tabs, TabsList, TabsTrigger } from "@orvilo/ui/components/ui/tabs";
import { useLocale, useT } from "../../i18n";

type DirectoryTab = "members" | "invitations";
type Density = "comfortable" | "compact";
type ColumnKey = "role" | "status" | "joined" | "invitedBy" | "sent";

const PAGE_SIZES = [5, 10, 20] as const;

function initialsFor(name: string): string {
  return name
    .trim()
    .split(/\s+/)
    .map((part) => part.charAt(0))
    .join("")
    .toUpperCase()
    .slice(0, 2) || "U";
}

function formatDate(value: string, locale: string): string {
  const date = new Date(value);
  return Number.isNaN(date.getTime())
    ? "—"
    : date.toLocaleDateString(locale, { year: "numeric", month: "short", day: "numeric" });
}

function MemberAvatar({ member }: { member: MemberWithUser }) {
  return (
    <Avatar className="size-8 shrink-0">
      {member.avatar_url ? <AvatarImage src={member.avatar_url} alt={member.name} /> : null}
      <AvatarFallback>{initialsFor(member.name)}</AvatarFallback>
    </Avatar>
  );
}

function InvitationAvatar({ invitation }: { invitation: Invitation }) {
  return (
    <span className="flex size-8 shrink-0 items-center justify-center rounded-full border border-dashed border-primary/35 bg-primary/10 text-primary">
      <Mail className="size-4" aria-hidden="true" />
      <span className="sr-only">{invitation.invitee_email}</span>
    </span>
  );
}

function StatusDot({ label, tone }: { label: string; tone: "active" | "pending" }) {
  return (
    <span className="inline-flex items-center gap-2 truncate text-sm">
      <span
        aria-hidden="true"
        className={tone === "active" ? "size-1.5 shrink-0 rounded-full bg-emerald-500" : "size-1.5 shrink-0 rounded-full bg-sky-500"}
      />
      {label}
    </span>
  );
}

function roleLabel(role: MemberRole, t: ReturnType<typeof useT<"teams">>["t"]): string {
  if (role === "owner") return t(($) => $.directory.owner_role);
  if (role === "admin") return t(($) => $.directory.admin_role);
  return t(($) => $.directory.member_role);
}

function DirectorySkeleton() {
  return (
    <Card className="mx-auto w-full max-w-[1440px] py-0">
      <CardHeader className="border-b px-5 py-5 md:px-6">
        <Skeleton className="h-6 w-32" />
        <Skeleton className="h-4 w-56" />
      </CardHeader>
      <div className="space-y-3 p-5 md:p-6">
        {Array.from({ length: 5 }, (_, index) => (
          <div key={index} className="flex items-center gap-4 border-b py-3 last:border-0">
            <Skeleton className="size-8 rounded-full" />
            <Skeleton className="h-4 w-36" />
            <Skeleton className="ml-auto h-4 w-40" />
          </div>
        ))}
      </div>
    </Card>
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
          <DialogDescription>{t(($) => $.directory.invite_description)}</DialogDescription>
        </DialogHeader>
        <form
          onSubmit={(event) => {
            event.preventDefault();
            if (valid && canManage && !isPending) onSubmit(email.trim(), role);
          }}
          className="space-y-4"
        >
          <label className="block space-y-1.5 text-sm font-medium" htmlFor="team-invite-email">
            {t(($) => $.directory.email_label)}
            <Input
              id="team-invite-email"
              type="email"
              autoFocus
              value={email}
              placeholder={t(($) => $.directory.email_placeholder)}
              onChange={(event) => setEmail(event.target.value)}
            />
          </label>
          <label className="block space-y-1.5 text-sm font-medium" htmlFor="team-invite-role">
            {t(($) => $.directory.role_label)}
            <Select
              items={[
                { value: "member", label: roleLabel("member", t) },
                { value: "admin", label: roleLabel("admin", t) },
              ]}
              value={role}
              onValueChange={(value) => setRole(value as MemberRole)}
            >
              <SelectTrigger id="team-invite-role" className="w-full">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="member">{roleLabel("member", t)}</SelectItem>
                <SelectItem value="admin">{roleLabel("admin", t)}</SelectItem>
              </SelectContent>
            </Select>
          </label>
          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => onOpenChange(false)}>
              {t(($) => $.directory.cancel)}
            </Button>
            <Button type="submit" disabled={!valid || isPending || !canManage}>
              <MailPlus className="size-4" aria-hidden="true" />
              {isPending ? t(($) => $.directory.inviting) : t(($) => $.directory.send_invite)}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

function ViewSettings({
  tab,
  density,
  onDensityChange,
  visible,
  onToggleColumn,
}: {
  tab: DirectoryTab;
  density: Density;
  onDensityChange: (density: Density) => void;
  visible: Record<ColumnKey, boolean>;
  onToggleColumn: (key: ColumnKey) => void;
}) {
  const { t } = useT("teams");
  const columns: { key: ColumnKey; label: string }[] = tab === "members"
    ? [
        { key: "role", label: t(($) => $.directory.role_column) },
        { key: "status", label: t(($) => $.directory.status_column) },
        { key: "joined", label: t(($) => $.directory.joined_column) },
      ]
    : [
        { key: "role", label: t(($) => $.directory.role_column) },
        { key: "invitedBy", label: t(($) => $.directory.invited_by_column) },
        { key: "sent", label: t(($) => $.directory.sent_column) },
      ];

  return (
    <Popover>
      <PopoverTrigger
        render={
          <Button type="button" variant="outline" size="sm" className="gap-1.5 text-muted-foreground">
            <Settings2 className="size-3.5" aria-hidden="true" />
            <span className="hidden sm:inline">{t(($) => $.directory.view_settings)}</span>
          </Button>
        }
      />
      <PopoverContent align="end" className="w-72 p-0">
        <div className="space-y-3 px-3.5 py-3">
          <div className="text-xs font-medium text-muted-foreground">{t(($) => $.directory.view_settings)}</div>
          <div className="flex items-center justify-between gap-3">
            <span className="text-sm">{t(($) => $.directory.density)}</span>
            <div className="flex rounded-md border p-0.5">
              {(["comfortable", "compact"] as const).map((option) => (
                <Button
                  key={option}
                  type="button"
                  size="xs"
                  variant={density === option ? "secondary" : "ghost"}
                  onClick={() => onDensityChange(option)}
                >
                  {option === "comfortable" ? "Comfortable" : "Compact"}
                </Button>
              ))}
            </div>
          </div>
          <Separator />
          <div className="space-y-2">
            <div className="text-xs font-medium text-muted-foreground">{t(($) => $.directory.columns)}</div>
            <div className="flex flex-wrap gap-1.5">
              {columns.map(({ key, label }) => (
                <Button
                  key={key}
                  type="button"
                  size="xs"
                  variant={visible[key] ? "secondary" : "outline"}
                  className="rounded-full"
                  onClick={() => onToggleColumn(key)}
                >
                  {visible[key] ? <Check className="size-3" aria-hidden="true" /> : null}
                  {label}
                </Button>
              ))}
            </div>
          </div>
        </div>
      </PopoverContent>
    </Popover>
  );
}

function MemberActions({
  member,
  canManage,
  onRoleChange,
  onRemove,
}: {
  member: MemberWithUser;
  canManage: boolean;
  onRoleChange: (role: MemberRole) => void;
  onRemove: () => void;
}) {
  const { t } = useT("teams");
  if (!canManage) return null;
  return (
    <DropdownMenu>
      <DropdownMenuTrigger
        render={<Button type="button" size="icon-sm" variant="ghost" aria-label={`${t(($) => $.directory.remove_member)} ${member.name}`} />}
      >
        <MoreHorizontal className="size-4" aria-hidden="true" />
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end">
        <DropdownMenuLabel>{t(($) => $.directory.role_column)}</DropdownMenuLabel>
        <DropdownMenuSub>
          <DropdownMenuSubTrigger>
            <Shield className="size-3.5" aria-hidden="true" />
            {roleLabel(member.role, t)}
          </DropdownMenuSubTrigger>
          <DropdownMenuSubContent>
            {(["member", "admin"] as const).map((role) => (
              <DropdownMenuItem key={role} onClick={() => onRoleChange(role)}>
                {member.role === role ? <Check className="size-3.5" aria-hidden="true" /> : <span className="size-3.5" />}
                {roleLabel(role, t)}
              </DropdownMenuItem>
            ))}
          </DropdownMenuSubContent>
        </DropdownMenuSub>
        <DropdownMenuSeparator />
        <DropdownMenuItem variant="destructive" onClick={onRemove}>
          <Trash2 className="size-3.5" aria-hidden="true" />
          {t(($) => $.directory.remove_member)}
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

function InvitationActions({
  invitation,
  canManage,
  onRevoke,
}: {
  invitation: Invitation;
  canManage: boolean;
  onRevoke: () => void;
}) {
  const { t } = useT("teams");
  if (!canManage) return null;
  return (
    <DropdownMenu>
      <DropdownMenuTrigger
        render={<Button type="button" size="icon-sm" variant="ghost" aria-label={`${t(($) => $.directory.revoke_invitation)} ${invitation.invitee_email}`} />}
      >
        <MoreHorizontal className="size-4" aria-hidden="true" />
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end">
        <DropdownMenuItem variant="destructive" onClick={onRevoke}>
          <X className="size-3.5" aria-hidden="true" />
          {t(($) => $.directory.revoke_invitation)}
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

export function TeamDirectoryPage() {
  const { t } = useT("teams");
  const locale = useLocale();
  const workspace = useCurrentWorkspace();
  const wsId = workspace?.id ?? "";
  const currentUser = useAuthStore((state) => state.user);
  const queryClient = useQueryClient();
  const [activeTab, setActiveTab] = useState<DirectoryTab>("members");
  const [search, setSearch] = useState("");
  const [inviteOpen, setInviteOpen] = useState(false);
  const [pageSize, setPageSize] = useState<(typeof PAGE_SIZES)[number]>(5);
  const [page, setPage] = useState(0);
  const [density, setDensity] = useState<Density>("comfortable");
  const [visibleColumns, setVisibleColumns] = useState<Record<ColumnKey, boolean>>({
    role: true,
    status: true,
    joined: true,
    invitedBy: true,
    sent: true,
  });

  const { data: members = [], isLoading: membersLoading } = useQuery({
    ...memberListOptions(wsId),
    enabled: !!wsId,
  });
  const { data: invitations = [], isLoading: invitationsLoading } = useQuery({
    ...invitationListOptions(wsId),
    enabled: !!wsId,
  });

  const currentMember = members.find((member) => member.user_id === currentUser?.id);
  const canManage = currentMember?.role === "owner" || currentMember?.role === "admin";
  const normalizedSearch = search.trim().toLowerCase();

  const filteredMembers = useMemo(
    () => members.filter((member) => `${member.name} ${member.email} ${member.role}`.toLowerCase().includes(normalizedSearch)),
    [members, normalizedSearch],
  );
  const filteredInvitations = useMemo(
    () => invitations.filter((invitation) => `${invitation.invitee_email} ${invitation.inviter_name ?? ""} ${invitation.role}`.toLowerCase().includes(normalizedSearch)),
    [invitations, normalizedSearch],
  );
  const rows = activeTab === "members" ? filteredMembers : filteredInvitations;
  const pageCount = Math.max(1, Math.ceil(rows.length / pageSize));
  const safePage = Math.min(page, pageCount - 1);
  const visibleRows = rows.slice(safePage * pageSize, (safePage + 1) * pageSize);

  useEffect(() => {
    setPage(0);
  }, [activeTab, normalizedSearch, pageSize]);

  const invalidateDirectory = async () => {
    await Promise.all([
      queryClient.invalidateQueries({ queryKey: workspaceKeys.members(wsId) }),
      queryClient.invalidateQueries({ queryKey: workspaceKeys.invitations(wsId) }),
    ]);
  };

  const inviteMutation = useMutation({
    mutationFn: async ({ email, role }: { email: string; role: MemberRole }) => {
      if (!workspace) throw new Error("Workspace is not ready");
      return api.createMember(workspace.id, { email, role });
    },
    onSuccess: async () => {
      await invalidateDirectory();
      setInviteOpen(false);
      toast.success(t(($) => $.directory.invite_sent));
    },
    onError: (error) => {
      const code = errorCode(error);
      toast.error(code ? `${t(($) => $.directory.invite_failed)} (${code})` : error instanceof Error ? error.message : t(($) => $.directory.invite_failed));
    },
  });

  const roleMutation = useMutation({
    mutationFn: async ({ memberId, role }: { memberId: string; role: MemberRole }) => {
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
    mutationFn: async (member: MemberWithUser) => {
      if (!workspace) throw new Error("Workspace is not ready");
      return api.deleteMember(workspace.id, member.id);
    },
    onSuccess: async () => {
      await invalidateDirectory();
      toast.success(t(($) => $.directory.remove_success));
    },
    onError: () => toast.error(t(($) => $.directory.action_failed)),
  });

  const revokeMutation = useMutation({
    mutationFn: async (invitation: Invitation) => {
      if (!workspace) throw new Error("Workspace is not ready");
      return api.revokeInvitation(workspace.id, invitation.id);
    },
    onSuccess: async () => {
      await invalidateDirectory();
      toast.success(t(($) => $.directory.revoke_success));
    },
    onError: () => toast.error(t(($) => $.directory.action_failed)),
  });

  if (!workspace) return null;

  const memberColumnVisible = (key: ColumnKey) => visibleColumns[key];
  const loading = membersLoading || invitationsLoading;

  return (
    <div className="flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto p-4 md:gap-5 md:p-6 lg:p-8">
      <div className="mx-auto flex w-full max-w-[1440px] items-center gap-2 text-sm text-muted-foreground">
        <span>{workspace.name}</span>
        <ChevronRight className="size-4" aria-hidden="true" />
        <span className="text-foreground">{t(($) => $.directory.title)}</span>
      </div>

      {loading ? (
        <DirectorySkeleton />
      ) : (
        <Card className="mx-auto w-full max-w-[1440px] py-0">
          <CardHeader className="border-b px-5 py-5 md:px-6">
            <div>
              <CardTitle className="text-xl">{t(($) => $.directory.title)}</CardTitle>
              <CardDescription className="mt-1 flex flex-wrap items-center gap-2">
                <span>{t(($) => $.directory.member_count, { count: members.length })}</span>
                <span aria-hidden="true">•</span>
                <span>{t(($) => $.directory.pending_invitation_count, { count: invitations.length })}</span>
              </CardDescription>
            </div>
            <CardAction className="flex gap-2">
              <Button type="button" variant="outline" size="sm" disabled={!canManage} onClick={() => setInviteOpen(true)}>
                <Plus className="size-3.5" aria-hidden="true" />
                <span className="hidden sm:inline">{t(($) => $.directory.add_member)}</span>
              </Button>
              <Button type="button" size="sm" disabled={!canManage} onClick={() => setInviteOpen(true)}>
                <MailPlus className="size-3.5" aria-hidden="true" />
                <span>{t(($) => $.directory.invite_people)}</span>
              </Button>
            </CardAction>
          </CardHeader>

          <Tabs value={activeTab} onValueChange={(value) => setActiveTab(value as DirectoryTab)}>
            <div className="border-b px-5 md:px-6">
              <TabsList variant="line" className="h-12 gap-5 rounded-none p-0">
                <TabsTrigger value="members" className="h-full flex-none px-0 text-sm">
                  {t(($) => $.directory.members_tab)}
                  <Badge variant={activeTab === "members" ? "secondary" : "outline"}>{members.length}</Badge>
                </TabsTrigger>
                <TabsTrigger value="invitations" className="h-full flex-none px-0 text-sm">
                  {t(($) => $.directory.invitations_tab)}
                  <Badge variant={activeTab === "invitations" ? "secondary" : "outline"}>{invitations.length}</Badge>
                </TabsTrigger>
              </TabsList>
            </div>

            <div className="flex flex-wrap items-center justify-between gap-3 border-b px-5 py-3 md:px-6">
              <div className="relative w-full max-w-sm">
                <Search className="pointer-events-none absolute left-2.5 top-1/2 size-4 -translate-y-1/2 text-muted-foreground" aria-hidden="true" />
                <Input
                  value={search}
                  onChange={(event) => setSearch(event.target.value)}
                  placeholder={activeTab === "members" ? t(($) => $.directory.search_members) : t(($) => $.directory.search_invitations)}
                  aria-label={activeTab === "members" ? t(($) => $.directory.search_members) : t(($) => $.directory.search_invitations)}
                  className="h-9 pl-9 pr-8"
                />
                {search ? (
                  <button type="button" aria-label={t(($) => $.directory.clear_search)} className="absolute right-2 top-1/2 -translate-y-1/2 text-muted-foreground hover:text-foreground" onClick={() => setSearch("")}>
                    <X className="size-3.5" aria-hidden="true" />
                  </button>
                ) : null}
              </div>
              <ViewSettings
                tab={activeTab}
                density={density}
                onDensityChange={setDensity}
                visible={visibleColumns}
                onToggleColumn={(key) => setVisibleColumns((current) => ({ ...current, [key]: !current[key] }))}
              />
            </div>

            <div className="overflow-x-auto">
              <Table className="min-w-[820px]">
                <TableHeader>
                  <TableRow className="hover:bg-transparent">
                    <TableHead className="w-[30%] px-5 md:px-6">
                      {activeTab === "members" ? t(($) => $.directory.member_column) : t(($) => $.directory.email_column)}
                    </TableHead>
                    {activeTab === "members" ? <TableHead className="w-[24%]">{t(($) => $.directory.email_column)}</TableHead> : null}
                    {memberColumnVisible("role") ? <TableHead>{t(($) => $.directory.role_column)}</TableHead> : null}
                    {activeTab === "members" && memberColumnVisible("status") ? <TableHead>{t(($) => $.directory.status_column)}</TableHead> : null}
                    {activeTab === "members" && memberColumnVisible("joined") ? <TableHead>{t(($) => $.directory.joined_column)}</TableHead> : null}
                    {activeTab === "invitations" && memberColumnVisible("invitedBy") ? <TableHead>{t(($) => $.directory.invited_by_column)}</TableHead> : null}
                    {activeTab === "invitations" && memberColumnVisible("sent") ? <TableHead>{t(($) => $.directory.sent_column)}</TableHead> : null}
                    <TableHead className="w-12 px-3"><span className="sr-only">{t(($) => $.directory.actions)}</span></TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {visibleRows.length === 0 ? (
                    <TableRow>
                      <TableCell colSpan={activeTab === "members" ? 7 : 5} className="h-36 text-center text-sm text-muted-foreground">
                        {activeTab === "members" ? t(($) => $.directory.no_members) : t(($) => $.directory.no_invitations)}
                      </TableCell>
                    </TableRow>
                  ) : activeTab === "members" ? (
                    (visibleRows as MemberWithUser[]).map((member) => (
                      <TableRow key={member.id} className="group/member-row">
                        <TableCell className={`${density === "compact" ? "py-2" : "py-3"} px-5 md:px-6`}>
                          <div className="flex min-w-0 items-center gap-2.5">
                            <MemberAvatar member={member} />
                            <div className="min-w-0">
                              <div className="truncate text-sm font-medium">{member.name}</div>
                              <div className="truncate text-sm text-muted-foreground">{member.user_id.slice(0, 8)}</div>
                            </div>
                          </div>
                        </TableCell>
                        <TableCell className="text-sm"><a className="hover:text-primary hover:underline" href={`mailto:${member.email}`}>{member.email}</a></TableCell>
                        {memberColumnVisible("role") ? <TableCell><Badge variant="outline">{roleLabel(member.role, t)}</Badge></TableCell> : null}
                        {memberColumnVisible("status") ? <TableCell><StatusDot label={t(($) => $.directory.active_status)} tone="active" /></TableCell> : null}
                        {memberColumnVisible("joined") ? <TableCell className="text-sm tabular-nums text-muted-foreground">{formatDate(member.created_at, locale)}</TableCell> : null}
                        <TableCell className="px-3 text-right">
                          {member.user_id === currentUser?.id ? null : (
                            <MemberActions
                              member={member}
                              canManage={canManage && member.role !== "owner"}
                              onRoleChange={(role) => roleMutation.mutate({ memberId: member.id, role })}
                              onRemove={() => removeMutation.mutate(member)}
                            />
                          )}
                        </TableCell>
                      </TableRow>
                    ))
                  ) : (
                    (visibleRows as Invitation[]).map((invitation) => (
                      <TableRow key={invitation.id} className="group/invitation-row">
                        <TableCell className={`${density === "compact" ? "py-2" : "py-3"} px-5 md:px-6`}>
                          <div className="flex min-w-0 items-center gap-2.5">
                            <InvitationAvatar invitation={invitation} />
                            <div className="min-w-0">
                              <div className="truncate text-sm font-medium">{invitation.invitee_email}</div>
                              <div className="flex items-center gap-1 text-sm text-muted-foreground"><Clock3 className="size-3" aria-hidden="true" />{t(($) => $.directory.pending_status)}</div>
                            </div>
                          </div>
                        </TableCell>
                        {memberColumnVisible("role") ? <TableCell><Badge variant="outline">{roleLabel(invitation.role, t)}</Badge></TableCell> : null}
                        {memberColumnVisible("invitedBy") ? <TableCell className="text-sm text-muted-foreground">{invitation.inviter_name ?? invitation.inviter_email ?? "—"}</TableCell> : null}
                        {memberColumnVisible("sent") ? <TableCell className="text-sm tabular-nums text-muted-foreground">{formatDate(invitation.created_at, locale)}</TableCell> : null}
                        <TableCell className="px-3 text-right">
                          <InvitationActions invitation={invitation} canManage={canManage} onRevoke={() => revokeMutation.mutate(invitation)} />
                        </TableCell>
                      </TableRow>
                    ))
                  )}
                </TableBody>
              </Table>
            </div>

            <Separator />
            <div className="flex flex-wrap items-center justify-between gap-3 px-5 py-3 text-sm text-muted-foreground md:px-6">
              <div className="flex items-center gap-2">
                <span>{t(($) => $.directory.rows_per_page)}</span>
                <Select
                  items={PAGE_SIZES.map((size) => ({ value: String(size), label: String(size) }))}
                  value={String(pageSize)}
                  onValueChange={(value) => setPageSize(Number(value) as (typeof PAGE_SIZES)[number])}
                >
                  <SelectTrigger size="sm" className="w-16"><SelectValue /></SelectTrigger>
                  <SelectContent>{PAGE_SIZES.map((size) => <SelectItem key={size} value={String(size)}>{size}</SelectItem>)}</SelectContent>
                </Select>
              </div>
              <div className="flex items-center gap-3">
                <span>
                  {rows.length === 0 ? "0" : `${safePage * pageSize + 1}–${Math.min((safePage + 1) * pageSize, rows.length)} ${t(($) => activeTab === "members" ? $.directory.range_members : $.directory.range_invitations)} of ${rows.length}`}
                </span>
                <div className="flex items-center gap-1">
                  <Button type="button" size="icon-sm" variant="ghost" aria-label={t(($) => $.directory.previous_page)} disabled={safePage === 0} onClick={() => setPage((current) => Math.max(0, current - 1))}><ChevronLeft className="size-4" aria-hidden="true" /></Button>
                  <span className="min-w-6 text-center tabular-nums">{safePage + 1}</span>
                  <Button type="button" size="icon-sm" variant="ghost" aria-label={t(($) => $.directory.next_page)} disabled={safePage >= pageCount - 1} onClick={() => setPage((current) => Math.min(pageCount - 1, current + 1))}><ChevronRight className="size-4" aria-hidden="true" /></Button>
                </div>
              </div>
            </div>
          </Tabs>
        </Card>
      )}

      <InviteDialog
        open={inviteOpen}
        onOpenChange={setInviteOpen}
        canManage={canManage}
        isPending={inviteMutation.isPending}
        onSubmit={(email, role) => inviteMutation.mutate({ email, role })}
      />
    </div>
  );
}
