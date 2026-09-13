"use client";

import { MessagingConnectionStatus } from "./messaging-connection-status";
import { MessagingSetupNotice, useMessagingSetupWritable } from "./messaging-setup-policy";

import { useEffect, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { AlertCircle, RefreshCw, Trash2 } from "lucide-react";
import { QRCode } from "react-qr-code";
import { toast } from "sonner";
// Two design systems in one file, and the split is by *surface* rather than by
// component. `WeixinTab` is Lobe, and it is safe because **both** of its hosts
// are bridged:
//
//   WeixinTab ← `./integrations-tab`'s channel dialog (`managedContent`)
//     └─ that component has exactly two importers, and only two:
//          settings-page.tsx        — inside <LobeThemeBridge> at its root
//          integrations/index.tsx   — WorkspaceIntegrationsPage, the Web
//                                     `/integrations` route; bridged since
//                                     b8a78232, and NOT before it
//
// Established by resolving the **import specifier**, not the identifier:
// `grep "<IntegrationsTab"` also matches
// `agents/components/tabs/integrations-tab.tsx` — a different component that
// shares the name and renders none of these tabs. A host list built from the
// name reports three hosts where there are two, which is why this comment names
// the importers rather than the export.
//
// `WeixinAgentBindButton` is rendered from the **agent detail page**
// (`packages/views/agents/components/tabs/integrations-tab.tsx`), so it keeps
// the shadcn primitives. That is not a *capability* boundary: the agent surface
// mounts its own `LobeThemeBridge`
// (`agents/components/agent-detail-page.tsx:323`), so the Lobe primitives it
// would need do render there. It stays shadcn because converting a control is a
// visual change — it needs its own decision and its own screenshot acceptance,
// and this round migrated the surface, not every control on it. The alias below
// is what keeps the two apart at the call sites.
//
// `WeixinInstallDialog` is the one component in this file that **straddles the
// boundary**: `WeixinTab` opens it and so does `WeixinAgentBindButton`, so it
// cannot be converted for one host alone — the conversion would change the
// controls on both. It stays on shadcn, reachable from the settings half as
// well, and that is the accepted intermediate state, not an oversight.
import { Button as LobeButton } from "@lobehub/ui/base-ui";
import { Button } from "@orvilo/ui/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@orvilo/ui/components/ui/dialog";
import { Input } from "@orvilo/ui/components/ui/input";
import {
  Field,
  FieldDescription,
  FieldError,
  FieldGroup,
  FieldLabel,
  FieldSeparator,
} from "@orvilo/ui/components/ui/field";
import { ApiError, api } from "@orvilo/core/api";
import { useAuthStore } from "@orvilo/core/auth";
import { useWorkspaceId } from "@orvilo/core/hooks";
import { agentListOptions, memberListOptions } from "@orvilo/core/workspace/queries";
import { weixinInstallationsOptions, weixinKeys } from "@orvilo/core/weixin";
import type { Agent, WeixinInstallation, WeixinInstallStatus } from "@orvilo/core/types";
import { useT } from "../../i18n";
import { useSettingsConfirm } from "./settings-confirm";
import { SettingsEmptyState } from "./settings-empty";
import { SettingsFormRow, SettingsGroup } from "./settings-shell";
import { WeixinMark } from "./weixin-mark";

type FlowStatus = WeixinInstallStatus | "error";

type InstallSession = {
  sessionId: string;
  qrCodeURL: string;
  pollIntervalSeconds: number;
};

function isWaitingStatus(status: FlowStatus): boolean {
  return status === "pending" || status === "scanned";
}

function errorReason(error: unknown): string {
  if (error instanceof ApiError) {
    if (error.status === 403) return "forbidden";
    if (error.status === 409) return "already_connected";
    if (error.status === 410) return "expired";
  }
  return "generic";
}

export function WeixinTab() {
  const setupWritable = useMessagingSetupWritable();
  const { t } = useT("settings");
  const wsId = useWorkspaceId();
  const qc = useQueryClient();
  const user = useAuthStore((s) => s.user);

  const { data: members = [] } = useQuery({
    ...memberListOptions(wsId),
    enabled: !!wsId,
  });
  const currentMember = members.find((member) => member.user_id === user?.id) ?? null;
  const isWorkspaceAdmin =
    currentMember?.role === "owner" || currentMember?.role === "admin";

  const { data: agents = [], isLoading: agentsLoading } = useQuery({
    ...agentListOptions(wsId),
    enabled: !!wsId,
  });
  const { data, isError, isLoading: installationsLoading } = useQuery({
    ...weixinInstallationsOptions(wsId),
    enabled: !!wsId,
  });

  const installations = data?.installations ?? [];
  const installedBotAgentIds = new Set(
    installations
      .filter((installation) => installation.status === "installed")
      .map((installation) => installation.agent_id),
  );
  const agentsById = new Map(agents.map((agent) => [agent.id, agent]));
  const canManageAgent = (agent: Agent) =>
    user?.is_guest !== true && (isWorkspaceAdmin || (currentMember != null && !!user?.id && agent.owner_id === user.id));
  const canManageInstallation = (installation: WeixinInstallation) => {
    const agent = agentsById.get(installation.agent_id);
    return agent ? canManageAgent(agent) : isWorkspaceAdmin;
  };
  const availableAgents = agents.filter(
    (agent) =>
      !agent.archived_at &&
      canManageAgent(agent) &&
      !installedBotAgentIds.has(agent.id),
  );

  const [connectAgent, setConnectAgent] = useState<Agent | null>(null);
  // The `disconnectTarget` / `disconnecting` pair of `useState`s is gone with
  // the `AlertDialog`: the imperative confirm owns both the open state and the
  // in-flight spinner, so the row hands its installation id straight through.
  const confirm = useSettingsConfirm();

  /**
   * Rejects on failure on purpose: `useSettingsConfirm`'s `onOk` keeps the
   * dialog open only while its promise is unsettled, so swallowing the error
   * here would dismiss the confirmation exactly when the account is still
   * connected. The toast is what the user reads; the rethrow is what holds the
   * dialog.
   */
  async function handleDisconnect(installationId: string) {
    if (!setupWritable) return;
    try {
      await api.deleteWeixinInstallation(wsId, installationId);
      await qc.invalidateQueries({ queryKey: weixinKeys.installations(wsId) });
      toast.success(t(($) => $.weixin.toast_disconnected));
    } catch (e) {
      toast.error(t(($) => $.weixin.toast_disconnect_failed));
      throw e;
    }
  }

  const openDisconnectConfirm = (installationId: string) =>
    confirm({
      title: t(($) => $.weixin.disconnect_confirm_title),
      description: t(($) => $.weixin.disconnect_confirm_description),
      confirmLabel: t(($) => $.weixin.disconnect),
      cancelLabel: t(($) => $.weixin.disconnect_confirm_cancel),
      onConfirm: () => handleDisconnect(installationId),
    });

  if (installationsLoading) {
    // The group's title is the state's name, not the page's: `installed_bots`
    // is "Weixin account installation records" in both locales, distinct from
    // the dialog's own title ("Manage" / "管理").
    return (
      <SettingsGroup variant="outlined" title={t(($) => $.weixin.installed_bots)}>
        <SettingsEmptyState title={t(($) => $.weixin.loading)} />
      </SettingsGroup>
    );
  }

  if (isError) {
    // The `Card` was only chrome around a notice, and there is no section
    // heading here to merge it with: an outlined group titled with this same
    // sentence would print it twice, and a group whose title is empty draws a
    // header band over its body.
    return (
      <div className="flex items-start gap-2">
        <AlertCircle className="mt-0.5 h-4 w-4 shrink-0 text-destructive" aria-hidden="true" />
        <p className="text-body text-muted-foreground">{t(($) => $.weixin.load_failed)}</p>
      </div>
    );
  }

  const configured = data?.configured === true;
  const installSupported = data?.install_supported === true;

  return (
    <div className="space-y-8">
      {!setupWritable && <MessagingSetupNotice />}
      {!configured && installations.length === 0 ? (
        <SettingsGroup variant="outlined" title={t(($) => $.weixin.not_enabled_title)}>
          <p className="text-body text-muted-foreground">
            {t(($) => $.weixin.not_enabled_description_prefix)}{" "}
            <code className="rounded bg-muted px-1 py-0.5 text-micro" translate="no">
              ORVILO_WEIXIN_SECRET_KEY
            </code>{" "}
            {t(($) => $.weixin.not_enabled_description_suffix)}{" "}
            {t(($) => $.weixin.not_enabled_self_host_hint)}
          </p>
        </SettingsGroup>
      ) : (
        <>
          {/* The `<section><h2>` heading and the `divide-y` card under it were
              two elements describing one section; a group's title is where a
              section heading goes, and the rows' `divider` is where the
              `divide-y` went. */}
          <SettingsGroup variant="outlined" title={t(($) => $.weixin.installed_bots)}>
            {installations.length === 0 ? (
              <SettingsEmptyState
                title={t(($) => $.weixin.empty_title)}
                description={t(($) => $.weixin.empty_description)}
              />
            ) : (
              installations.map((installation, index) => (
                <InstallationRow
                  key={installation.id}
                  divider={index > 0}
                  installation={installation}
                  agentName={installation.agent_id
                    ? agentsById.get(installation.agent_id)?.name ?? t(($) => $.weixin.unknown_agent)
                    : t(($) => $.page.integrations_workspace_connection)}
                  canManage={canManageInstallation(installation) && setupWritable}
                  onDisconnect={() => openDisconnectConfirm(installation.id)}
                />
              ))
            )}
          </SettingsGroup>

          {installSupported && setupWritable ? (
            <SettingsGroup
              variant="outlined"
              title={t(($) => $.weixin.available_agents)}
              description={t(($) => $.weixin.available_agents_description)}
            >
              {agentsLoading ? (
                <SettingsEmptyState title={t(($) => $.weixin.loading)} />
              ) : availableAgents.length === 0 ? (
                <SettingsEmptyState title={t(($) => $.weixin.no_available_agents)} />
              ) : (
                availableAgents.map((agent, index) => (
                  <SettingsFormRow
                    key={agent.id}
                    divider={index > 0}
                    label={
                      <span className="inline-flex min-w-0 items-center gap-2">
                        <WeixinMark className="h-5 w-5 shrink-0 text-muted-foreground" />
                        <span className="min-w-0 truncate">{agent.name}</span>
                      </span>
                    }
                  >
                    {/* `type="primary"`: the base control was a shadcn
                        `<Button>` with no `variant`, which is `bg-primary
                        text-primary-foreground` — the loudest control in this
                        row, and the row's whole purpose. Lobe's default `type`
                        is the outlined treatment, so omitting this silently
                        demotes the CTA to a secondary button. The same action
                        on the agent detail page is still solid primary. */}
                    <LobeButton
                      type="primary"
                      onClick={() => setConnectAgent(agent)}
                      title={t(($) => $.weixin.connect_button_title, { agent: agent.name })}
                      data-testid={`weixin-connect-agent-${agent.id}`}
                    >
                      <WeixinMark className="h-3 w-3" />
                      {t(($) => $.weixin.connect_button)}
                    </LobeButton>
                  </SettingsFormRow>
                ))
              )}
            </SettingsGroup>
          ) : installations.length === 0 ? (
            <SettingsGroup variant="outlined" title={t(($) => $.weixin.preview_title)}>
              <p className="text-body text-muted-foreground">
                {t(($) => $.weixin.preview_description)}
              </p>
            </SettingsGroup>
          ) : null}
        </>
      )}

      {connectAgent && setupWritable ? (
        <WeixinInstallDialog
          wsId={wsId}
          agentId={connectAgent.id}
          agentName={connectAgent.name}
          onClose={() => setConnectAgent(null)}
        />
      ) : null}
    </div>
  );
}

export function WeixinAgentBindButton({
  agentId,
  agentName,
}: {
  agentId?: string;
  agentName?: string;
}) {
  const { t } = useT("settings");
  const wsId = useWorkspaceId();
  const setupWritable = useMessagingSetupWritable();
  const user = useAuthStore((state) => state.user);
  const [open, setOpen] = useState(false);
  const { data: members = [] } = useQuery({ ...memberListOptions(wsId), enabled: !!wsId });
  const { data: agents = [] } = useQuery({ ...agentListOptions(wsId), enabled: !!wsId && !!agentId });
  const { data: listing, isError } = useQuery({ ...weixinInstallationsOptions(wsId), enabled: !!wsId });
  const member = members.find((item) => item.user_id === user?.id);
  const canManage = member?.role === "owner" || member?.role === "admin" ||
    (member != null && !!user?.id && agents.some((agent) => agent.id === agentId && agent.owner_id === user.id));
  if (!canManage || user?.is_guest === true) return null;
  const existing = listing?.installations.find((installation) =>
    (agentId ? installation.agent_id === agentId : !installation.agent_id) && installation.status === "installed",
  );
  if (existing) {
    return <MessagingConnectionStatus installation={isError ? { ...existing, runtime: undefined } : existing} compact />;
  }
  if (!setupWritable) return <MessagingSetupNotice />;
  if (listing?.install_supported !== true || isError) return null;
  return (
    <>
      <Button
        type="button"
        disabled={!wsId}
        onClick={() => setOpen(true)}
        title={agentName ? t(($) => $.weixin.connect_button_title, { agent: agentName }) : undefined}
      >
        <WeixinMark className="h-3 w-3" />
        {t(($) => $.weixin.connect_button)}
      </Button>
      {open ? (
        <WeixinInstallDialog
          wsId={wsId}
          agentId={agentId}
          agentName={agentName}
          onClose={() => setOpen(false)}
        />
      ) : null}
    </>
  );
}

/**
 * One installation, as a `SettingsFormRow`: the identity is the row's label,
 * the connection state and bot id are its description, and the destructive
 * action is the control column. `divider` replaces the old `divide-y` on the
 * card that used to hold these rows. `MessagingConnectionStatus` keeps the
 * full (non-compact) form this tab already rendered.
 */
function InstallationRow({
  installation,
  agentName,
  canManage,
  divider,
  onDisconnect,
}: {
  installation: WeixinInstallation;
  agentName: string;
  canManage: boolean;
  divider: boolean;
  onDisconnect: () => void;
}) {
  const { t } = useT("settings");
  const isInstalled = installation.status === "installed";
  return (
    <SettingsFormRow
      divider={divider}
      label={
        <span className="inline-flex min-w-0 items-center gap-2">
          <WeixinMark className="h-5 w-5 shrink-0 text-muted-foreground" />
          <span className="min-w-0 truncate">{agentName}</span>
          {!isInstalled ? (
            <span className="shrink-0 rounded bg-muted px-1.5 py-0.5 text-micro text-muted-foreground">
              {t(($) => $.weixin.revoked_badge)}
            </span>
          ) : null}
        </span>
      }
      description={
        <>
          <MessagingConnectionStatus installation={installation} />
          <span className="block truncate text-micro text-muted-foreground">
            {t(($) => $.weixin.bot_id_label)}{" "}
            <code translate="no">{installation.bot_id || t(($) => $.weixin.unknown_bot)}</code>
          </span>
        </>
      }
    >
      {canManage && isInstalled ? (
        <LobeButton onClick={onDisconnect}>
          <Trash2 className="h-3 w-3" />
          {t(($) => $.weixin.disconnect)}
        </LobeButton>
      ) : null}
    </SettingsFormRow>
  );
}

function WeixinInstallDialog({
  wsId,
  agentId,
  agentName,
  onClose,
}: {
  wsId: string;
  agentId?: string;
  agentName?: string;
  onClose: () => void;
}) {
  const { t } = useT("settings");
  const qc = useQueryClient();
  const [session, setSession] = useState<InstallSession | null>(null);
  const [status, setStatus] = useState<FlowStatus>("pending");
  const [errorKind, setErrorKind] = useState<string | null>(null);
  const [starting, setStarting] = useState(false);
  const [verifyCode, setVerifyCode] = useState("");
  const [verifying, setVerifying] = useState(false);
  const closedRef = useRef(false);
  const startedRef = useRef(false);
  const startingRef = useRef(false);
  const closeTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  async function finishInstall() {
    if (closedRef.current) return;
    setStatus("success");
    await qc.invalidateQueries({ queryKey: weixinKeys.installations(wsId) });
    if (closedRef.current) return;
    toast.success(t(($) => $.weixin.install_success_toast));
    closeTimerRef.current = setTimeout(() => {
      if (!closedRef.current) onClose();
    }, 700);
  }

  async function beginSession() {
    if (startingRef.current || closedRef.current) return;
    startingRef.current = true;
    setStarting(true);
    setSession(null);
    setStatus("pending");
    setErrorKind(null);
    setVerifyCode("");
    try {
      const response = await api.beginWeixinInstall(wsId, agentId);
      if (closedRef.current) return;
      if (!response.session_id || !response.qr_code_url) {
        setStatus("error");
        setErrorKind("generic");
        return;
      }
      setSession({
        sessionId: response.session_id,
        qrCodeURL: response.qr_code_url,
        pollIntervalSeconds: response.poll_interval_seconds,
      });
    } catch (error) {
      if (!closedRef.current) {
        setStatus("error");
        setErrorKind(errorReason(error));
      }
    } finally {
      startingRef.current = false;
      if (!closedRef.current) setStarting(false);
    }
  }

  useEffect(() => {
    closedRef.current = false;
    // React StrictMode replays effects in development. Keep the one-shot
    // authorization request idempotent while still allowing the explicit
    // Retry action to call beginSession again.
    if (!startedRef.current) {
      startedRef.current = true;
      void beginSession();
    }
    return () => {
      closedRef.current = true;
      if (closeTimerRef.current) clearTimeout(closeTimerRef.current);
    };
    // The dialog owns this one-shot flow; beginSession intentionally runs once.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    if (!session || !isWaitingStatus(status)) return;
    const intervalMs = Math.max(2000, session.pollIntervalSeconds * 1000);
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | null = null;

    const poll = async () => {
      if (cancelled) return;
      try {
        const response = await api.getWeixinInstallStatus(wsId, session.sessionId);
        if (cancelled || closedRef.current) return;
        if (response.status === "success") {
          await finishInstall();
          return;
        }
        if (isWaitingStatus(response.status)) {
          setStatus(response.status);
          timer = setTimeout(poll, intervalMs);
          return;
        }
        if (response.status === "need_verify_code") {
          setStatus(response.status);
          setErrorKind(null);
          return;
        }
        setStatus("error");
        setErrorKind(
          response.status === "already_connected" || response.status === "expired"
            ? response.status
            : "generic",
        );
      } catch (error) {
        if (cancelled || closedRef.current) return;
        if (error instanceof ApiError && [401, 403, 409, 410].includes(error.status)) {
          setStatus("error");
          setErrorKind(errorReason(error));
          return;
        }
        timer = setTimeout(poll, intervalMs);
      }
    };

    timer = setTimeout(poll, intervalMs);
    return () => {
      cancelled = true;
      if (timer) clearTimeout(timer);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [session?.sessionId, status]);

  async function handleVerify(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const code = verifyCode.trim();
    if (!session || !code || verifying) return;
    setVerifying(true);
    setErrorKind(null);
    try {
      const response = await api.getWeixinInstallStatus(wsId, session.sessionId, code);
      if (closedRef.current) return;
      if (response.status === "success") {
        await finishInstall();
      } else if (isWaitingStatus(response.status)) {
        setStatus(response.status);
      } else if (response.status === "need_verify_code") {
        setStatus(response.status);
        setErrorKind("verify_invalid");
      } else {
        setStatus("error");
        setErrorKind(
          response.status === "already_connected" || response.status === "expired"
            ? response.status
            : "generic",
        );
      }
    } catch (error) {
      if (closedRef.current) return;
      if (error instanceof ApiError && [401, 403, 409, 410].includes(error.status)) {
        setStatus("error");
        setErrorKind(errorReason(error));
      } else {
        setErrorKind("generic");
      }
    } finally {
      if (!closedRef.current) setVerifying(false);
    }
  }

  function errorCopy() {
    switch (errorKind) {
      case "expired":
        return t(($) => $.weixin.install_error_expired);
      case "already_connected":
        return t(($) => $.weixin.install_error_already_connected);
      case "forbidden":
        return t(($) => $.weixin.install_error_forbidden);
      case "verify_invalid":
        return t(($) => $.weixin.install_error_verify_invalid);
      default:
        return t(($) => $.weixin.install_error_generic);
    }
  }

  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="max-w-sm" data-testid="weixin-install-dialog">
        <DialogHeader>
          <DialogTitle>{t(($) => $.weixin.connect_dialog_title)}</DialogTitle>
          <DialogDescription>
            {agentName
              ? t(($) => $.weixin.connect_dialog_description, { agent: agentName })
              : t(($) => $.weixin.install_scan_hint)}
          </DialogDescription>
        </DialogHeader>

        <div className="flex flex-col items-center gap-4 py-2" aria-live="polite">
          {starting && !session ? (
            <p className="text-body text-muted-foreground">{t(($) => $.weixin.install_starting)}</p>
          ) : null}

          {session && (status === "pending" || status === "scanned") ? (
            <>
              <div className="rounded-md border bg-white p-3">
                <QRCode
                  value={session.qrCodeURL}
                  size={192}
                  aria-label={t(($) => $.weixin.qr_code_label)}
                />
              </div>
              <p className="text-center text-caption text-muted-foreground">
                {status === "scanned"
                  ? t(($) => $.weixin.install_scanned)
                  : t(($) => $.weixin.install_scan_hint)}
              </p>
              <a
                href={session.qrCodeURL}
                target="_blank"
                rel="noopener noreferrer"
                className="text-caption text-muted-foreground underline underline-offset-2 hover:text-foreground"
              >
                {t(($) => $.weixin.install_open_link)}
              </a>
            </>
          ) : null}

          {status === "need_verify_code" && session ? (
            <form
              className="w-full"
              onSubmit={handleVerify}
            >
              <p className="mb-4 text-body font-medium">{t(($) => $.weixin.verify_title)}</p>
              <FieldGroup>
                <Field
                  orientation="responsive"
                  data-invalid={errorKind === "verify_invalid" ? true : undefined}
                  data-disabled={verifying ? true : undefined}
                >
                  <FieldLabel htmlFor="weixin-verify-code">
                    {t(($) => $.weixin.verify_code_label)}
                  </FieldLabel>
                  <Input
                    id="weixin-verify-code"
                    name="weixin_verification_code"
                    data-testid="weixin-verify-code"
                    value={verifyCode}
                    onChange={(event) => setVerifyCode(event.target.value)}
                    placeholder={t(($) => $.weixin.verify_code_placeholder)}
                    autoComplete="one-time-code"
                    inputMode="numeric"
                    spellCheck={false}
                    disabled={verifying}
                    aria-invalid={errorKind === "verify_invalid" ? true : undefined}
                  />
                  <FieldDescription>{t(($) => $.weixin.verify_description)}</FieldDescription>
                  {errorKind === "verify_invalid" ? (
                    <FieldError>{t(($) => $.weixin.install_error_verify_invalid)}</FieldError>
                  ) : null}
                </Field>
                <FieldSeparator />
                <div className="flex justify-end gap-2">
                  <Button type="submit" disabled={!verifyCode.trim() || verifying}>
                    {verifying ? t(($) => $.weixin.verifying) : t(($) => $.weixin.verify_submit)}
                  </Button>
                </div>
              </FieldGroup>
            </form>
          ) : null}

          {status === "success" ? (
            <p className="text-body font-medium">{t(($) => $.weixin.install_success)}</p>
          ) : null}

          {status === "error" ? (
            <p className="text-center text-body font-medium text-destructive">{errorCopy()}</p>
          ) : null}
        </div>

        <DialogFooter>
          {status === "error" ? (
            <>
              <Button variant="outline" size="sm" onClick={onClose}>{t(($) => $.weixin.install_close)}</Button>
              <Button size="sm" onClick={beginSession} disabled={starting}>
                <RefreshCw className="h-3 w-3" />
                {t(($) => $.weixin.install_retry)}
              </Button>
            </>
          ) : (
            <Button variant="outline" size="sm" onClick={onClose}>{t(($) => $.weixin.install_close)}</Button>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
