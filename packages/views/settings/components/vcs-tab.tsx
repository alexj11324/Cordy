"use client";

/**
 * Self-hosted Git providers (Forgejo / Gitea / GitLab).
 *
 * **This tab had no test file at all** until Task 8b added
 * `vcs-tab.test.tsx`, which is why both traps below were first answered by
 * reading the source rather than by a red suite. That suite now guards the two
 * of them — the accessible names of the connect-form controls, and the absence
 * of the shadcn props Lobe would have silently forwarded to the DOM.
 *
 * 1. The three connect-form fields were `<Label htmlFor>` + a control carrying
 *    the matching `id`. The mapping deletes `Label`, and deleting it without
 *    moving the association would have left all three controls unnamed with no
 *    test to notice. They are `SettingsFormRow`s with `htmlFor` now, which is
 *    the prop the shell exists to offer for exactly this: antd mints a label's
 *    `for` only from a field `name`, and the state-ownership rule forbids one.
 * 2. The row buttons were `<Button variant="outline" size="sm">`. Lobe's
 *    `Button` has no `variant` and its `size` union has no `"sm"`, and
 *    `Button.mjs` spreads `...rest` onto the DOM, so `variant` would have
 *    reached the DOM as an invalid attribute with no error at all. `outline` is
 *    Lobe's default treatment (nothing to pass) and `size` keeps the default
 *    `middle` per the F5 ruling.
 *
 * The outer `SettingsGroup` is owned by `integrations-tab.tsx`, which renders
 * this tab as the body of the "Git providers (self-hosted)" section — so the
 * three baseline `Card`s became rows and prose inside that one group rather
 * than three more groups.
 */

import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { Copy, GitBranch, RefreshCw, Trash2 } from "lucide-react";
import { Button, Input } from "@lobehub/ui/base-ui";
import { useWorkspaceId } from "@orvilo/core/hooks";
import { vcsConnectionsOptions } from "@orvilo/core/vcs";
import { api } from "@orvilo/core/api";
import type { ConnectVCSResponse, VCSProvider } from "@orvilo/core/types";
import { useT } from "../../i18n";
import { useSettingsConfirm } from "./settings-confirm";
import { SettingsSelect } from "./settings-select";
import { SettingsFormRow } from "./settings-shell";

const PROVIDERS: VCSProvider[] = ["forgejo", "gitea", "gitlab"];
const PROVIDER_LABELS: Record<VCSProvider, string> = {
  forgejo: "Forgejo",
  gitea: "Gitea",
  gitlab: "GitLab",
};
const PROVIDER_OPTIONS = PROVIDERS.map((p) => ({
  value: p,
  label: PROVIDER_LABELS[p],
}));

export function VCSTab() {
  const { t } = useT("settings");
  const wsId = useWorkspaceId();
  const qc = useQueryClient();
  // The `rotateTarget` / `rotating` and `deleteTarget` / `deleting` pairs of
  // `useState`s are gone with the two `AlertDialog`s: the imperative confirm
  // owns the open state and the in-flight spinner, so the rows hand their
  // connection id straight through.
  const confirm = useSettingsConfirm();

  const { data } = useQuery(vcsConnectionsOptions(wsId));
  const connections = data?.connections ?? [];
  const configured = data?.configured === true;
  const canManage = data?.can_manage === true;

  const [provider, setProvider] = useState<VCSProvider>("forgejo");
  const [instanceUrl, setInstanceUrl] = useState("");
  const [token, setToken] = useState("");
  const [connecting, setConnecting] = useState(false);
  const [justConnected, setJustConnected] = useState<ConnectVCSResponse | null>(null);

  async function handleConnect() {
    if (connecting || !instanceUrl.trim() || !token.trim()) return;
    setConnecting(true);
    try {
      const resp = await api.connectVCS(wsId, {
        provider,
        instance_url: instanceUrl.trim(),
        access_token: token.trim(),
      });
      await qc.invalidateQueries({ queryKey: ["vcs", wsId] });
      setJustConnected(resp);
      setInstanceUrl("");
      setToken("");
      toast.success(t(($) => $.vcs.toast_connected));
    } catch (e) {
      toast.error(e instanceof Error ? e.message : t(($) => $.vcs.toast_connect_failed));
    } finally {
      setConnecting(false);
    }
  }

  // Both handlers return their promise and rethrow on failure — the caller half
  // of `confirmModal`'s contract, which is what keeps the dialog open when the
  // request fails instead of closing on a row that never changed.
  async function handleRotateWebhook(connectionId: string) {
    try {
      const resp = await api.rotateVCSWebhook(wsId, connectionId);
      await qc.invalidateQueries({ queryKey: ["vcs", wsId] });
      setJustConnected(resp);
      toast.success(t(($) => $.vcs.toast_rotated));
    } catch (e) {
      toast.error(e instanceof Error ? e.message : t(($) => $.vcs.toast_rotate_failed));
      throw e;
    }
  }

  async function handleDelete(connectionId: string) {
    try {
      await api.deleteVCSConnection(wsId, connectionId);
      await qc.invalidateQueries({ queryKey: ["vcs", wsId] });
      if (justConnected?.id === connectionId) setJustConnected(null);
      toast.success(t(($) => $.vcs.toast_disconnected));
    } catch (e) {
      toast.error(e instanceof Error ? e.message : t(($) => $.vcs.toast_disconnect_failed));
      throw e;
    }
  }

  const openRotateConfirm = (connectionId: string) =>
    confirm({
      title: t(($) => $.vcs.rotate_confirm_title),
      description: t(($) => $.vcs.rotate_confirm_description),
      confirmLabel: t(($) => $.vcs.rotate_confirm_action),
      cancelLabel: t(($) => $.vcs.rotate_confirm_cancel),
      onConfirm: () => handleRotateWebhook(connectionId),
    });

  const openDeleteConfirm = (connectionId: string) =>
    confirm({
      title: t(($) => $.vcs.disconnect_confirm_title),
      description: t(($) => $.vcs.disconnect_confirm_description),
      confirmLabel: t(($) => $.vcs.disconnect_confirm_action),
      cancelLabel: t(($) => $.vcs.disconnect_confirm_cancel),
      onConfirm: () => handleDelete(connectionId),
    });

  async function copy(value: string) {
    try {
      await navigator.clipboard.writeText(value);
      toast.success(t(($) => $.vcs.copied));
    } catch {
      toast.error(t(($) => $.vcs.copy_failed));
    }
  }

  return (
    <div className="space-y-6">
      <p className="text-body text-muted-foreground">{t(($) => $.vcs.page_description)}</p>

      {connections.map((c, index) => (
        <SettingsFormRow
          key={c.id}
          align="start"
          divider={index > 0}
          label={
            <span className="inline-flex min-w-0 items-center gap-2 break-all">
              <GitBranch className="size-4 shrink-0 text-muted-foreground" />
              {(PROVIDER_LABELS[c.provider] ?? c.provider) + " · " + c.instance_url}
            </span>
          }
          description={t(($) => $.vcs.connected_as, { login: c.account_login })}
        >
          {canManage ? (
            <div className="flex flex-wrap items-center gap-2">
              {/* Both were `<Button variant="outline" size="sm">` — Lobe's
                  bordered default at the default `middle` size. */}
              <Button
                icon={<RefreshCw className="size-3.5" />}
                shape="round"
                onClick={() => openRotateConfirm(c.id)}
              >
                {t(($) => $.vcs.regenerate_webhook)}
              </Button>
              <Button
                icon={<Trash2 className="size-3.5" />}
                shape="round"
                onClick={() => openDeleteConfirm(c.id)}
              >
                {t(($) => $.vcs.disconnect)}
              </Button>
            </div>
          ) : null}
        </SettingsFormRow>
      ))}

      {justConnected ? (
        <div className="space-y-4 border-t border-border pt-4">
          <div className="space-y-1">
            <p className="text-body font-medium">{t(($) => $.vcs.webhook_setup_title)}</p>
            <p className="text-caption text-muted-foreground">
              {t(($) => $.vcs.webhook_setup_description)}
            </p>
          </div>
          <CopyField
            id="vcs-webhook-url"
            label={t(($) => $.vcs.webhook_url_label)}
            value={justConnected.webhook_url || justConnected.webhook_path}
            onCopy={copy}
            copyLabel={t(($) => $.vcs.copy)}
          />
          <CopyField
            id="vcs-webhook-secret"
            label={t(($) => $.vcs.webhook_secret_label)}
            value={justConnected.webhook_secret}
            onCopy={copy}
            copyLabel={t(($) => $.vcs.copy)}
            mono
          />
          <p className="text-caption text-amber-600 dark:text-amber-500">
            {t(($) => $.vcs.webhook_secret_warning)}
          </p>
        </div>
      ) : null}

      {canManage ? (
        <div className="space-y-4 border-t border-border pt-4">
          <p className="text-body font-medium">{t(($) => $.vcs.connect_title)}</p>
          {!configured ? (
            <p className="text-caption text-muted-foreground">
              {t(($) => $.vcs.not_configured)}{" "}
              <code className="rounded bg-muted px-1 py-0.5 text-micro">
                ORVILO_VCS_SECRET_KEY
              </code>
              .
            </p>
          ) : (
            <>
              {/* `htmlFor` + the control's `id` is what keeps the accessible
                  name the deleted `<Label htmlFor>` used to provide. */}
              <SettingsFormRow
                htmlFor="vcs-provider"
                label={t(($) => $.vcs.form_provider_label)}
              >
                <SettingsSelect
                  className="w-full"
                  disabled={connecting}
                  id="vcs-provider"
                  label={t(($) => $.vcs.form_provider_label)}
                  options={PROVIDER_OPTIONS}
                  value={provider}
                  onValueChange={(v) => setProvider(v as VCSProvider)}
                />
              </SettingsFormRow>
              <SettingsFormRow
                htmlFor="vcs-url"
                label={t(($) => $.vcs.form_instance_url_label)}
              >
                <Input
                  disabled={connecting}
                  id="vcs-url"
                  placeholder="https://forgejo.example.com"
                  value={instanceUrl}
                  onChange={(e) => setInstanceUrl(e.target.value)}
                />
              </SettingsFormRow>
              <SettingsFormRow
                htmlFor="vcs-token"
                label={t(($) => $.vcs.form_token_label)}
                description={t(($) => $.vcs.form_token_hint)}
              >
                <Input
                  disabled={connecting}
                  id="vcs-token"
                  placeholder={t(($) => $.vcs.form_token_placeholder)}
                  type="password"
                  value={token}
                  onChange={(e) => setToken(e.target.value)}
                />
              </SettingsFormRow>
              <div className="flex justify-end">
                {/* The baseline was `<Button size="sm">` with no `variant` —
                    shadcn's solid primary, which is Lobe's `type="primary"`.
                    Only the box height moves (28 -> 32), per the F5 ruling. */}
                <Button
                  disabled={connecting || !instanceUrl.trim() || !token.trim()}
                  shape="round"
                  type="primary"
                  onClick={handleConnect}
                >
                  {connecting ? t(($) => $.vcs.connecting) : t(($) => $.vcs.connect)}
                </Button>
              </div>
            </>
          )}
        </div>
      ) : null}

      {!canManage && connections.length === 0 && (
        <p className="text-caption text-muted-foreground">{t(($) => $.vcs.contact_admin)}</p>
      )}
    </div>
  );
}

function CopyField({
  id,
  label,
  value,
  onCopy,
  copyLabel,
  mono,
}: {
  id: string;
  label: string;
  value: string;
  onCopy: (v: string) => void;
  copyLabel: string;
  mono?: boolean;
}) {
  return (
    <SettingsFormRow align="start" htmlFor={id} label={label}>
      <div className="flex items-center gap-2">
        <Input
          readOnly
          id={id}
          value={value}
          className={mono ? "min-w-0 font-mono" : "min-w-0"}
        />
        <Button
          icon={<Copy className="size-3.5" />}
          shape="round"
          title={copyLabel}
          onClick={() => onCopy(value)}
        />
      </div>
    </SettingsFormRow>
  );
}
