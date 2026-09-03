"use client";

import { MessagingConnectionStatus } from "./messaging-connection-status";

import { useEffect, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { AlertCircle, RefreshCw, Trash2 } from "lucide-react";
import { QRCode } from "react-qr-code";
import { toast } from "sonner";
import { AlertDialog, AlertDialogAction, AlertDialogCancel, AlertDialogContent, AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle } from "@patchbay/ui/components/ui/alert-dialog";
import { Button } from "@patchbay/ui/components/ui/button";
import { Card, CardContent } from "@patchbay/ui/components/ui/card";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@patchbay/ui/components/ui/dialog";
import { Input } from "@patchbay/ui/components/ui/input";
import { Label } from "@patchbay/ui/components/ui/label";
import { ApiError, api } from "@patchbay/core/api";
import { useAuthStore } from "@patchbay/core/auth";
import { useWorkspaceId } from "@patchbay/core/hooks";
import { agentListOptions, memberListOptions } from "@patchbay/core/workspace/queries";
import { weixinInstallationsOptions, weixinKeys } from "@patchbay/core/weixin";
import type { Agent, WeixinInstallation, WeixinInstallStatus } from "@patchbay/core/types";
import { useT } from "../../i18n";
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
    isWorkspaceAdmin || (!!user?.id && agent.owner_id === user.id);
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

  const [disconnectTarget, setDisconnectTarget] = useState<string | null>(null);
  const [connectAgent, setConnectAgent] = useState<Agent | null>(null);
  const [disconnecting, setDisconnecting] = useState(false);

  async function handleDisconnect() {
    if (!disconnectTarget || disconnecting) return;
    setDisconnecting(true);
    try {
      await api.deleteWeixinInstallation(wsId, disconnectTarget);
      await qc.invalidateQueries({ queryKey: weixinKeys.installations(wsId) });
      toast.success(t(($) => $.weixin.toast_disconnected));
      setDisconnectTarget(null);
    } catch {
      toast.error(t(($) => $.weixin.toast_disconnect_failed));
    } finally {
      setDisconnecting(false);
    }
  }

  if (installationsLoading) {
    return (
      <Card>
        <CardContent>
          <p className="text-body text-muted-foreground">{t(($) => $.weixin.loading)}</p>
        </CardContent>
      </Card>
    );
  }

  if (isError) {
    return (
      <Card>
        <CardContent className="flex items-start gap-2">
          <AlertCircle className="mt-0.5 h-4 w-4 shrink-0 text-destructive" aria-hidden="true" />
          <p className="text-body text-muted-foreground">{t(($) => $.weixin.load_failed)}</p>
        </CardContent>
      </Card>
    );
  }

  const configured = data?.configured === true;
  const installSupported = data?.install_supported === true;

  return (
    <div className="space-y-8">
      {!configured ? (
        <Card>
          <CardContent className="space-y-2">
            <p className="text-body font-medium">{t(($) => $.weixin.not_enabled_title)}</p>
            <p className="text-caption text-muted-foreground">
              {t(($) => $.weixin.not_enabled_description_prefix)}{" "}
              <code className="rounded bg-muted px-1 py-0.5 text-micro" translate="no">
                PATCHBAY_WEIXIN_SECRET_KEY
              </code>{" "}
              {t(($) => $.weixin.not_enabled_description_suffix)}{" "}
              {t(($) => $.weixin.not_enabled_self_host_hint)}
            </p>
          </CardContent>
        </Card>
      ) : (
        <>
          <section className="space-y-3">
            <h2 className="text-body font-semibold">{t(($) => $.weixin.installed_bots)}</h2>
            {installations.length === 0 ? (
              <Card>
                <CardContent className="space-y-2">
                  <p className="text-body font-medium">{t(($) => $.weixin.empty_title)}</p>
                  <p className="text-caption text-muted-foreground">{t(($) => $.weixin.empty_description)}</p>
                </CardContent>
              </Card>
            ) : (
              <Card>
                <CardContent className="divide-y">
                  {installations.map((installation) => (
                    <InstallationRow
                      key={installation.id}
                      installation={installation}
                      agentName={agentsById.get(installation.agent_id)?.name ?? t(($) => $.weixin.unknown_agent)}
                      canManage={canManageInstallation(installation)}
                      onDisconnect={() => setDisconnectTarget(installation.id)}
                    />
                  ))}
                </CardContent>
              </Card>
            )}
          </section>

          {installSupported ? (
            <section className="space-y-3">
              <h2 className="text-body font-semibold">{t(($) => $.weixin.available_agents)}</h2>
              <p className="text-caption text-muted-foreground">
                {t(($) => $.weixin.available_agents_description)}
              </p>
              {agentsLoading ? (
                <p className="text-body text-muted-foreground">{t(($) => $.weixin.loading)}</p>
              ) : availableAgents.length === 0 ? (
                <Card>
                  <CardContent>
                    <p className="text-caption text-muted-foreground">{t(($) => $.weixin.no_available_agents)}</p>
                  </CardContent>
                </Card>
              ) : (
                <Card>
                  <CardContent className="divide-y">
                    {availableAgents.map((agent) => (
                      <div key={agent.id} className="flex items-center justify-between gap-4 py-3 first:pt-0 last:pb-0">
                        <div className="flex min-w-0 items-center gap-3">
                          <WeixinMark className="h-5 w-5 shrink-0 text-muted-foreground" />
                          <p className="min-w-0 truncate text-body font-medium">{agent.name}</p>
                        </div>
                        <Button
                          variant="outline"
                          size="sm"
                          onClick={() => setConnectAgent(agent)}
                          title={t(($) => $.weixin.connect_button_title, { agent: agent.name })}
                          data-testid={`weixin-connect-agent-${agent.id}`}
                        >
                          <WeixinMark className="h-3 w-3" />
                          {t(($) => $.weixin.connect_button)}
                        </Button>
                      </div>
                    ))}
                  </CardContent>
                </Card>
              )}
            </section>
          ) : installations.length === 0 ? (
            <Card>
              <CardContent className="space-y-2">
                <p className="text-body font-medium">{t(($) => $.weixin.preview_title)}</p>
                <p className="text-caption text-muted-foreground">{t(($) => $.weixin.preview_description)}</p>
              </CardContent>
            </Card>
          ) : null}
        </>
      )}

      <AlertDialog
        open={!!disconnectTarget}
        onOpenChange={(open) => {
          if (!open && !disconnecting) setDisconnectTarget(null);
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{t(($) => $.weixin.disconnect_confirm_title)}</AlertDialogTitle>
            <AlertDialogDescription>{t(($) => $.weixin.disconnect_confirm_description)}</AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={disconnecting}>{t(($) => $.weixin.disconnect_confirm_cancel)}</AlertDialogCancel>
            <AlertDialogAction onClick={handleDisconnect} disabled={disconnecting}>
              {disconnecting ? t(($) => $.weixin.disconnecting) : t(($) => $.weixin.disconnect)}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

      {connectAgent ? (
        <WeixinInstallDialog
          wsId={wsId}
          agent={connectAgent}
          onClose={() => setConnectAgent(null)}
        />
      ) : null}
    </div>
  );
}

function InstallationRow({
  installation,
  agentName,
  canManage,
  onDisconnect,
}: {
  installation: WeixinInstallation;
  agentName: string;
  canManage: boolean;
  onDisconnect: () => void;
}) {
  const { t } = useT("settings");
  const isInstalled = installation.status === "installed";
  return (
    <div className="flex items-start justify-between gap-4 py-3 first:pt-0 last:pb-0">
      <div className="flex min-w-0 items-start gap-3">
        <WeixinMark className="mt-0.5 h-5 w-5 shrink-0 text-muted-foreground" />
        <div className="min-w-0 space-y-1">
          <MessagingConnectionStatus installation={installation} />
          <p className="truncate text-body font-medium">
            {agentName}
            {!isInstalled && (
              <span className="ml-2 rounded bg-muted px-1.5 py-0.5 text-micro text-muted-foreground">
                {t(($) => $.weixin.revoked_badge)}
              </span>
            )}
          </p>
          <p className="truncate text-micro text-muted-foreground">
            {t(($) => $.weixin.bot_id_label)}{" "}
            <code translate="no">{installation.bot_id || t(($) => $.weixin.unknown_bot)}</code>
          </p>
        </div>
      </div>
      {canManage && isInstalled ? (
        <Button variant="outline" size="sm" onClick={onDisconnect}>
          <Trash2 className="h-3 w-3" />
          {t(($) => $.weixin.disconnect)}
        </Button>
      ) : null}
    </div>
  );
}

function WeixinInstallDialog({
  wsId,
  agent,
  onClose,
}: {
  wsId: string;
  agent: Agent;
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
      const response = await api.beginWeixinInstall(wsId, agent.id);
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
            {t(($) => $.weixin.connect_dialog_description, { agent: agent.name })}
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
            <form className="w-full space-y-3" onSubmit={handleVerify}>
              <div className="space-y-1.5">
                <p className="text-body font-medium">{t(($) => $.weixin.verify_title)}</p>
                <p className="text-caption text-muted-foreground">{t(($) => $.weixin.verify_description)}</p>
                <Label htmlFor="weixin-verify-code">{t(($) => $.weixin.verify_code_label)}</Label>
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
                />
                {errorKind === "verify_invalid" ? (
                  <p className="text-caption text-destructive">{t(($) => $.weixin.install_error_verify_invalid)}</p>
                ) : null}
              </div>
              <Button type="submit" className="w-full" disabled={!verifyCode.trim() || verifying}>
                {verifying ? t(($) => $.weixin.verifying) : t(($) => $.weixin.verify_submit)}
              </Button>
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
