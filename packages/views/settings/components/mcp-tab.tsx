"use client";

import { useMemo, useState } from "react";
import { Loader2, Plus } from "lucide-react";
import { useQuery } from "@tanstack/react-query";
import { toast } from "sonner";
import { Button } from "@lobehub/ui/base-ui";
import { Badge } from "@orvilo/ui/components/ui/badge";
import { useCurrentWorkspace } from "@orvilo/core/paths";
import { useCurrentMember } from "@orvilo/core/permissions";
import { workspaceMcpServersOptions } from "@orvilo/core/workspace/queries";
import {
  useCreateWorkspaceMcpServer,
  useDeleteWorkspaceMcpServer,
  useUpdateWorkspaceMcpServer,
} from "@orvilo/core/workspace/mutations";
import type { WorkspaceMcpServer } from "@orvilo/core/types";
import { McpServerDialog } from "../../agents/components/tabs/mcp-server-dialog";
import type { ManagedMcpServer } from "../../agents/components/tabs/mcp-config-model";
import { useT } from "../../i18n";
import { useSettingsConfirm } from "./settings-confirm";
import { SettingsEmptyState } from "./settings-empty";
import { SettingsFormRow, SettingsGroup } from "./settings-shell";

/**
 * The workspace MCP server library (GH #6062).
 *
 * Two things shape this screen and are worth stating up front:
 *
 *  - A server added here is given to every agent in this workspace on its
 *    next task. There is no per-agent assignment step. Native CLI MCP,
 *    plugin MCP, and Composio overlays still layer on after this library.
 *  - The stored configuration is WRITE-ONLY. The API returns names and
 *    transports, never urls / commands / headers / env, so there is no
 *    "current value" to prefill and editing a server means supplying its
 *    configuration again. The UI says so rather than pretending the empty form
 *    is the saved state.
 *
 * **`McpServerDialog` stays shadcn this round, and that is a decision rather
 * than an oversight.** It is also rendered by
 * `agents/components/tabs/mcp-config-tab.tsx` on the agent surface, so a
 * conversion has to land on both surfaces at once. The agent surface mounts
 * `LobeThemeBridge` now (`agents/components/agent-detail-page.tsx`), so a
 * converted dialog would render there rather than break — but converting a
 * shared dialog is a visual change that needs its own decision and its own
 * screenshot acceptance. This tab migrated; the dialog it renders did not.
 */
export function McpTab() {
  const { t } = useT("settings");
  const workspace = useCurrentWorkspace();
  const wsId = workspace?.id ?? "";
  const currentMember = useCurrentMember(wsId);
  const canManage =
    currentMember.role === "owner" || currentMember.role === "admin";

  const serversQuery = useQuery(workspaceMcpServersOptions(wsId));
  const createServer = useCreateWorkspaceMcpServer(wsId);
  const updateServer = useUpdateWorkspaceMcpServer(wsId);
  const deleteServer = useDeleteWorkspaceMcpServer(wsId);
  // The `deletingServer` state and its `AlertDialog` are gone: the imperative
  // confirm owns the open state and the in-flight spinner, so the row hands the
  // server straight through.
  const confirm = useSettingsConfirm();

  const servers = serversQuery.data ?? [];
  const existingNames = useMemo(
    () => new Set(servers.map((server) => server.name)),
    [servers],
  );

  const [editorOpen, setEditorOpen] = useState(false);
  const [editingServer, setEditingServer] = useState<WorkspaceMcpServer | null>(
    null,
  );

  // The dialog is shared with the agent MCP tab, which hands it the saved
  // entry to prefill. Here there is nothing to prefill — an edit always
  // starts from an empty form and REPLACES the entry. The transport still
  // comes from the safe summary so the form opens on the right one.
  const dialogServer: ManagedMcpServer | null = editingServer
    ? {
        name: editingServer.name,
        config: {},
        container: "mcpServers",
        transport: editingServer.transport,
        // The library has no per-agent toggle; this field only feeds the
        // dialog's shape.
        enabled: true,
      }
    : null;

  const handleSaveServer = async (
    name: string,
    config: Record<string, unknown>,
  ) => {
    try {
      if (editingServer) {
        // Renaming is safe here: assignments key off the server id, so an
        // agent that uses this server keeps using it.
        await updateServer.mutateAsync({ serverId: editingServer.id, name, config });
      } else {
        await createServer.mutateAsync({ name, config });
      }
      toast.success(
        editingServer
          ? t(($) => $.mcp.updated_toast)
          : t(($) => $.mcp.added_toast),
      );
    } catch (error) {
      toast.error(
        error instanceof Error && error.message
          ? error.message
          : t(($) => $.mcp.save_failed_toast),
      );
      throw error;
    }
  };

  // Returns its promise and rethrows on failure — the caller half of the
  // `confirmModal` contract, which is what keeps the confirmation open when the
  // request fails instead of dismissing as though the row were gone. Swallowing
  // the rejection here would close the dialog on a server that still exists.
  const handleDelete = async (server: WorkspaceMcpServer) => {
    try {
      await deleteServer.mutateAsync(server.id);
      toast.success(t(($) => $.mcp.removed_toast));
    } catch (error) {
      toast.error(
        error instanceof Error && error.message
          ? error.message
          : t(($) => $.mcp.remove_failed_toast),
      );
      throw error;
    }
  };

  const openDeleteConfirm = (server: WorkspaceMcpServer) =>
    confirm({
      title: t(($) => $.mcp.delete_title),
      description: t(($) => $.mcp.delete_description, { name: server.name }),
      confirmLabel: t(($) => $.mcp.delete_confirm),
      cancelLabel: t(($) => $.mcp.cancel),
      onConfirm: () => handleDelete(server),
    });

  // `SettingsTab` is gone, and with it the page heading. Its `title` was already
  // dropped inside the settings dialog — `settings-page.tsx`'s `DialogHeader`
  // renders `page.tabs.mcp`, and `mcp.title` resolves to the same string, so it
  // must not come back as a group title. Its `description` has no such home
  // (`tabDescription("mcp")` returns `""`), so it stays with the panel as the
  // lede, the placement `billing-tab` and `vcs-tab` gave theirs.
  //
  // `action`: this tab has no page-level action. `mcp-tab.tsx:146`'s old
  // `action=` was passed to `SettingsSection`, which does not read the dialog
  // context — so it was never dropped, and it moves to the group's `extra`
  // because that is the mapping, not because anything was lost.
  return (
    <div className="space-y-8">
      <p className="text-body text-muted-foreground">{t(($) => $.mcp.description)}</p>

      <SettingsGroup
        // `desc` renders as a `<small>` in the group's header, so the scale the
        // old section paragraph carried has to travel on the node itself.
        description={
          <span className="block max-w-3xl text-body leading-relaxed text-muted-foreground">
            {t(($) => $.mcp.write_only_note)}
          </span>
        }
        extra={
          canManage ? (
            // `SettingsPillButton active` resolved its tone to `primary`, which
            // is Lobe's `type="primary"`; `shape="round"` is the pill geometry.
            <Button
              icon={<Plus className="size-4" />}
              shape="round"
              type="primary"
              onClick={() => {
                setEditingServer(null);
                setEditorOpen(true);
              }}
            >
              {t(($) => $.mcp.add_server)}
            </Button>
          ) : undefined
        }
        title={t(($) => $.mcp.servers_title)}
      >
        {serversQuery.isLoading ? (
          <div className="flex items-center justify-center py-8 text-muted-foreground">
            <Loader2 className="h-4 w-4 animate-spin" />
          </div>
        ) : servers.length === 0 ? (
          <SettingsEmptyState
            description={t(($) => $.mcp.empty_description)}
            title={t(($) => $.mcp.empty_title)}
          />
        ) : (
          servers.map((server, index) => (
            <McpServerRow
              key={server.name}
              canManage={canManage}
              // The old `SettingsCard` drew its `divide-y` between rows; the
              // group's rows draw their own.
              divider={index > 0}
              server={server}
              onDelete={() => openDeleteConfirm(server)}
              onEdit={() => {
                setEditingServer(server);
                setEditorOpen(true);
              }}
            />
          ))
        )}
        {!canManage && !currentMember.isLoading ? (
          <p className="pt-3 text-caption text-muted-foreground">
            {t(($) => $.mcp.admin_only_note)}
          </p>
        ) : null}
      </SettingsGroup>

      <McpServerDialog
        open={editorOpen}
        server={dialogServer}
        existingNames={existingNames}
        onOpenChange={setEditorOpen}
        onSave={handleSaveServer}
      />
    </div>
  );
}

function McpServerRow({
  server,
  canManage,
  divider,
  onEdit,
  onDelete,
}: {
  server: WorkspaceMcpServer;
  canManage: boolean;
  divider: boolean;
  onEdit: () => void;
  onDelete: () => void;
}) {
  const { t } = useT("settings");
  // The baseline was `SettingsListRow`, and it is inlined rather than re-homed:
  // a row of `Form.Item` gives the name a `<label>` slot and the actions a
  // control slot, which is what the old flex row already expressed — plus the
  // antd geometry (`base.css`) the settings rows share.
  //
  // **A button under a row label is renamed by one of exactly two routes, and
  // this row is on neither.** The first is being *inside* the label's subtree:
  // `dom-accessibility-api`'s `getControlOfLabel` falls back to
  // `findLabelableElement(label)` — the first labelable descendant — and
  // `isLabelableElement` counts a `button` as labelable. These two sit in the
  // control column, a sibling of the label. The second is a `for=`
  // association, which this row cannot have because it passes no `htmlFor` —
  // the prop `SettingsFormRow` exposes for naming a text field, and the reason
  // a row that *does* pass it must not put a button in its control slot.
  //
  // Measured on the rendered row, not reasoned: `computeAccessibleName` is
  // "Edit server", and both `insideLabelSubtree` and `namedByForLabel` are
  // false. Each button also carries its own `aria-label`, which the name
  // computation consults before any `<label>` (step 2C returns before 2D in
  // `accessible-name-and-description.mjs`) — that is belt and braces here, not
  // what the row depends on.
  return (
    <SettingsFormRow
      description={transportLabel(server.transport)}
      divider={divider}
      label={
        <span className="inline-flex min-w-0 items-center gap-2">
          <span className="truncate text-body font-medium">{server.name}</span>
          {server.enabled === false ? (
            <Badge variant="secondary">{t(($) => $.mcp.disabled_badge)}</Badge>
          ) : null}
        </span>
      }
    >
      {canManage ? (
        <div className="flex shrink-0 items-center gap-1.5">
          {/* `SettingsPillButton` with no `tone` was the `muted` pill —
              `bg-muted` on a transparent border, which is Lobe's `fill`. */}
          <Button
            aria-label={t(($) => $.mcp.edit_server)}
            shape="round"
            type="fill"
            onClick={onEdit}
          >
            {t(($) => $.mcp.edit_server)}
          </Button>
          {/* `tone="destructive"` — `type="fill"` + `danger`, the destructive
              fill rather than the solid red a primary-danger button gives. */}
          <Button
            aria-label={t(($) => $.mcp.remove_server)}
            danger
            shape="round"
            type="fill"
            onClick={onDelete}
          >
            {t(($) => $.mcp.remove_server)}
          </Button>
        </div>
      ) : null}
    </SettingsFormRow>
  );
}

/**
 * `transport` is a server-driven string, so an unknown value from a newer
 * backend renders as itself instead of disappearing.
 */
function transportLabel(transport: string): string {
  switch (transport) {
    case "stdio":
      return "stdio";
    case "http":
      return "HTTP";
    case "sse":
      return "SSE";
    default:
      return transport || "unknown";
  }
}
