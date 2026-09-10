"use client";

import { useMemo, useState } from "react";
import {
  AlertCircle,
  Brain,
  History,
  ListFilter,
  Plug,
  Plus,
  Search,
} from "lucide-react";
import { useQuery } from "@tanstack/react-query";
import { automationListOptions } from "@orvilo/core/automations/queries";
import { parseAutomationTools } from "@orvilo/core/automations";
import { useAuthStore } from "@orvilo/core/auth";
import { useWorkspaceId } from "@orvilo/core/hooks";
import { useWorkspacePaths } from "@orvilo/core/paths";
import { useActorName } from "@orvilo/core/workspace/hooks";
import type { Automation } from "@orvilo/core/types";
import { Button, buttonVariants } from "@orvilo/ui/components/ui/button";
import { Checkbox } from "@orvilo/ui/components/ui/checkbox";
import { Input } from "@orvilo/ui/components/ui/input";
import { Tabs, TabsList, TabsTrigger } from "@orvilo/ui/components/ui/tabs";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@orvilo/ui/components/ui/table";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuCheckboxItem,
  DropdownMenuTrigger,
} from "@orvilo/ui/components/ui/dropdown-menu";
import { Skeleton } from "@orvilo/ui/components/ui/skeleton";
import { AppLink, useNavigation, useRowLink } from "../../navigation";
import { SlackMark } from "../../settings/components/slack-mark";
import { ActorAvatar } from "../../common/actor-avatar";
import {
  CollectionPageHeaderAction,
  CollectionPageState,
} from "../../layout/collection-page";
import { ShellHeaderActions } from "../../layout/shell-header";
import {
  AutomationBatchToolbar,
  AutomationRowActions,
} from "./automation-list-actions";
import { AutomationTemplateGallery } from "./automation-template-gallery";
import type { AutomationTemplate } from "./automation-templates";
import { useT } from "../../i18n";

function ConfiguredTools({ automation }: { automation: Automation }) {
  const { t } = useT("automations");
  const tools = parseAutomationTools(automation.tools);
  const hasMemories = tools.memories && tools.memories.enabled !== false;
  const hasSlack = tools.slack_send && tools.slack_send.enabled !== false;
  const mcpCount = Array.isArray(tools.mcp_server_ids)
    ? tools.mcp_server_ids.length
    : 0;
  return (
    <span className="flex items-center gap-2 text-muted-foreground">
      {hasMemories ? (
        <Brain
          className="size-4"
          aria-label={t(($) => $.settings.tools_memories)}
        />
      ) : null}
      {hasSlack ? (
        <span title={t(($) => $.settings.tools_slack)}>
          <SlackMark className="size-4" />
        </span>
      ) : null}
      {mcpCount > 0 ? (
        <span
          className="flex items-center gap-1"
          title={t(($) => $.settings.tools_mcp)}
        >
          <Plug className="size-4" />
          <span className="text-caption">{mcpCount}</span>
        </span>
      ) : null}
      {!hasMemories && !hasSlack && mcpCount === 0 ? "—" : null}
    </span>
  );
}

export function AutomationsPage() {
  const { t } = useT("automations");
  const wsId = useWorkspaceId();
  const wsPaths = useWorkspacePaths();
  const navigation = useNavigation();
  const rowLink = useRowLink();
  const currentUser = useAuthStore((s) => s.user);
  const { getActorName } = useActorName();
  const {
    data: automations = [],
    isLoading,
    error: listError,
    refetch: refetchList,
  } = useQuery(automationListOptions(wsId));
  const [selectedIds, setSelectedIds] = useState<ReadonlySet<string>>(
    new Set(),
  );
  const [ownership, setOwnership] = useState("team");
  const [searchOpen, setSearchOpen] = useState(false);
  const [search, setSearch] = useState("");
  const [status, setStatus] = useState<"all" | "active" | "paused">("all");
  const [page, setPage] = useState(0);
  const rows = useMemo(
    () =>
      automations
        .filter(
          (a) =>
            a.status !== "archived" &&
            (ownership !== "mine" ||
              (a.created_by_type === "member" &&
                a.created_by_id === currentUser?.id)) &&
            (status === "all" || a.status === status) &&
            a.title
              .toLocaleLowerCase()
              .includes(search.trim().toLocaleLowerCase()),
        )
        .sort((a, b) => Date.parse(b.created_at) - Date.parse(a.created_at)),
    [automations, ownership, currentUser?.id, status, search],
  );
  const currentPage = Math.min(
    page,
    Math.max(0, Math.ceil(rows.length / 25) - 1),
  );
  const pageRows = rows.slice(currentPage * 25, currentPage * 25 + 25);
  const selectedRows = rows.filter((a) => selectedIds.has(a.id));
  const allSelected =
    pageRows.length > 0 && pageRows.every((a) => selectedIds.has(a.id));
  const toggleSelected = (id: string) =>
    setSelectedIds((previous) => {
      const next = new Set(previous);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  const openCreate = (template?: AutomationTemplate) =>
    navigation.push(wsPaths.newAutomation(template?.id));
  return (
    <div className="relative flex min-h-0 flex-1 flex-col">
      <ShellHeaderActions>
        <CollectionPageHeaderAction
          icon={Plus}
          label={t(($) => $.page.new_automation)}
          onClick={() => openCreate()}
        />
      </ShellHeaderActions>
      {listError ? (
        <CollectionPageState
          role="alert"
          tone="destructive"
          icon={AlertCircle}
          title={
            listError instanceof Error ? listError.message : String(listError)
          }
          actions={
            <Button
              variant="outline"
              size="sm"
              onClick={() => void refetchList()}
            >
              {t(($) => $.page.retry)}
            </Button>
          }
        />
      ) : isLoading ? (
        <div className="mx-auto w-full max-w-4xl space-y-3 p-6">
          {[0, 1, 2].map((id) => (
            <Skeleton key={id} className="h-14 w-full" />
          ))}
        </div>
      ) : (
        <div className="min-h-0 flex-1 overflow-y-auto">
          <section
            aria-labelledby="your-automations-heading"
            className="mx-auto w-full max-w-4xl px-5 pb-8 pt-8"
          >
            <h2 id="your-automations-heading" className="sr-only">
              {t(($) => $.page.your_automations)}
            </h2>
            <div className="mb-4 flex flex-wrap items-center gap-2">
              <Tabs
                value={ownership}
                onValueChange={(value) => {
                  setOwnership(value);
                  setPage(0);
                  setSelectedIds(new Set());
                }}
                className="mr-auto"
              >
                <TabsList>
                  <TabsTrigger value="mine">
                    {t(($) => $.overview.mine)}
                  </TabsTrigger>
                  <TabsTrigger value="team">
                    {t(($) => $.overview.team)}
                  </TabsTrigger>
                </TabsList>
              </Tabs>
              <AppLink
                href={wsPaths.automationRuns()}
                className={buttonVariants({ variant: "ghost", size: "sm" })}
              >
                <History />
                {t(($) => $.overview.all_runs)}
              </AppLink>
              {searchOpen ? (
                <Input
                  autoFocus
                  className="h-8 w-44"
                  value={search}
                  onChange={(event) => {
                    setSearch(event.target.value);
                    setPage(0);
                  }}
                  placeholder={t(($) => $.overview.search_automations)}
                  aria-label={t(($) => $.overview.search_automations)}
                />
              ) : null}
              <Button
                variant="ghost"
                size="icon-sm"
                aria-label={t(($) => $.overview.search_automations)}
                aria-pressed={searchOpen}
                onClick={() => {
                  setSearchOpen(!searchOpen);
                  setSearch("");
                  setPage(0);
                }}
              >
                <Search />
              </Button>
              <DropdownMenu>
                <DropdownMenuTrigger
                  render={
                    <Button
                      variant={status === "all" ? "ghost" : "secondary"}
                      size="icon-sm"
                      aria-label={t(($) => $.overview.filter_automations)}
                    />
                  }
                >
                  <ListFilter />
                </DropdownMenuTrigger>
                <DropdownMenuContent align="end">
                  {(["all", "active", "paused"] as const).map((value) => (
                    <DropdownMenuCheckboxItem
                      key={value}
                      checked={status === value}
                      onCheckedChange={() => {
                        setStatus(value);
                        setPage(0);
                      }}
                    >
                      {value === "all"
                        ? t(($) => $.overview.all_statuses)
                        : t(($) => $.status[value])}
                    </DropdownMenuCheckboxItem>
                  ))}
                </DropdownMenuContent>
              </DropdownMenu>
            </div>
            <div className="overflow-hidden rounded-xl border bg-background">
              <Table>
                <TableHeader>
                  <TableRow className="hover:bg-transparent">
                    <TableHead className="w-9 pl-4">
                      <Checkbox
                        aria-label={t(($) => $.overview.select_all)}
                        checked={allSelected}
                        indeterminate={
                          pageRows.some((a) => selectedIds.has(a.id)) &&
                          !allSelected
                        }
                        onCheckedChange={() =>
                          setSelectedIds((previous) => {
                            const next = new Set(previous);
                            for (const a of pageRows) {
                              if (allSelected) next.delete(a.id);
                              else next.add(a.id);
                            }
                            return next;
                          })
                        }
                      />
                    </TableHead>
                    <TableHead>{t(($) => $.page.table.name)}</TableHead>
                    <TableHead>{t(($) => $.page.table.created_by)}</TableHead>
                    <TableHead>{t(($) => $.run_history.status)}</TableHead>
                    <TableHead>{t(($) => $.run_history.tools)}</TableHead>
                    <TableHead className="w-10">
                      <span className="sr-only">
                        {t(($) => $.run_history.actions)}
                      </span>
                    </TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {pageRows.map((automation) => (
                    <TableRow
                      key={automation.id}
                      className="group/row h-14 cursor-pointer"
                      {...rowLink(
                        wsPaths.automationDetail(automation.id),
                        automation.title,
                      )}
                    >
                      <TableCell
                        className="pl-4"
                        onClick={(event) => event.stopPropagation()}
                      >
                        <Checkbox
                          aria-label={automation.title}
                          checked={selectedIds.has(automation.id)}
                          onCheckedChange={() => toggleSelected(automation.id)}
                        />
                      </TableCell>
                      <TableCell className="max-w-80">
                        <span className="block truncate font-medium">
                          {automation.title}
                        </span>
                      </TableCell>
                      <TableCell>
                        <span className="flex items-center gap-2 text-muted-foreground">
                          <ActorAvatar
                            actorType={automation.created_by_type}
                            actorId={automation.created_by_id}
                            size="sm"
                          />
                          <span className="max-w-32 truncate">
                            {getActorName(
                              automation.created_by_type,
                              automation.created_by_id,
                            )}
                          </span>
                        </span>
                      </TableCell>
                      <TableCell>
                        <span className="inline-flex items-center gap-1.5 rounded-md border px-2 py-0.5 text-caption">
                          <span
                            className={`size-1.5 rounded-full ${automation.status === "active" ? "bg-emerald-500" : "bg-muted-foreground"}`}
                          />
                          {automation.status === "active"
                            ? t(($) => $.status.active)
                            : t(($) => $.detail.status_inactive)}
                        </span>
                      </TableCell>
                      <TableCell>
                        <ConfiguredTools automation={automation} />
                      </TableCell>
                      <TableCell onClick={(event) => event.stopPropagation()}>
                        <AutomationRowActions row={automation} />
                      </TableCell>
                    </TableRow>
                  ))}
                  {rows.length === 0 ? (
                    <TableRow>
                      <TableCell
                        colSpan={6}
                        className="h-28 text-center text-muted-foreground"
                      >
                        {t(($) => $.page.no_matches)}
                      </TableCell>
                    </TableRow>
                  ) : null}
                </TableBody>
              </Table>
            </div>
            {rows.length > 25 ? (
              <div className="mt-3 flex items-center justify-end gap-3 text-caption">
                <Button
                  variant="ghost"
                  size="sm"
                  disabled={currentPage === 0}
                  onClick={() => setPage(currentPage - 1)}
                >
                  {t(($) => $.overview.previous)}
                </Button>
                <span>
                  {currentPage + 1} / {Math.ceil(rows.length / 25)}
                </span>
                <Button
                  variant="ghost"
                  size="sm"
                  disabled={(currentPage + 1) * 25 >= rows.length}
                  onClick={() => setPage(currentPage + 1)}
                >
                  {t(($) => $.overview.next)}
                </Button>
              </div>
            ) : null}
          </section>
          <AutomationTemplateGallery
            onSelectTemplate={openCreate}
            onStartBlank={() => openCreate()}
            persistent
          />
        </div>
      )}
      {selectedRows.length > 0 ? (
        <AutomationBatchToolbar
          rows={selectedRows}
          onClear={() => setSelectedIds(new Set())}
        />
      ) : null}
    </div>
  );
}
