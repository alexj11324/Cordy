"use client";

import { useEffect, useState } from "react";
import { QRCode } from "react-qr-code";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { Trash2 } from "lucide-react";
import { Button } from "@patchbay/ui/components/ui/button";
import { Card, CardContent } from "@patchbay/ui/components/ui/card";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@patchbay/ui/components/ui/dialog";
import { SettingsInput as Input } from "@patchbay/ui/components/common/lobe-settings";
import { useWorkspaceId } from "@patchbay/core/hooks";
import { useAuthStore } from "@patchbay/core/auth";
import { memberListOptions } from "@patchbay/core/workspace/queries";
import { api } from "@patchbay/core/api";
import { isMessagingInstallationHealthy } from "@patchbay/core/types";
import { weixinInstallationsOptions, weixinKeys } from "@patchbay/core/weixin";
import { WeixinMark } from "./weixin-mark";
import { useT } from "../../i18n";

function qrImageSource(value: string): string | null {
  const trimmed = value.trim();
  if (/^data:image\//i.test(trimmed) || /^https:\/\//i.test(trimmed)) {
    return trimmed;
  }
  // Some iLink deployments return raw base64 PNG content. Keep the fallback
  // explicit so an opaque polling token can never be mistaken for an image.
  if (/^[A-Za-z0-9+/]+=*$/.test(trimmed) && trimmed.length > 128) {
    return `data:image/png;base64,${trimmed}`;
  }
  return null;
}

function providerErrorDetail(error: unknown): string | null {
  const detail = error instanceof Error ? error.message.trim() : "";
  if (!detail) return null;
  return detail
    .replace(
      /(token|secret|authorization)\s*[:=]\s*(?:bearer\s+)?[^\s,;]+/gi,
      "$1=[redacted]",
    )
    .replace(/\bbearer\s+[^\s,;]+/gi, "bearer [redacted]")
    .slice(0, 240);
}

export function WeixinTab({ installationId }: { installationId?: string } = {}) {
  const { t } = useT("settings");
  const wsId = useWorkspaceId();
  const qc = useQueryClient();
  const user = useAuthStore((state) => state.user);
  const { data: members = [] } = useQuery(memberListOptions(wsId));
  const role = members.find((member) => member.user_id === user?.id)?.role;
  const canManage = role === "owner" || role === "admin";
  const { data, isLoading, isError } = useQuery(
    weixinInstallationsOptions(wsId),
  );
  async function disconnect(id: string) {
    try {
      await api.deleteWeixinInstallation(wsId, id);
      await qc.invalidateQueries({ queryKey: weixinKeys.installations(wsId) });
      toast.success(t(($) => $.weixin.disconnected));
    } catch (error) {
      const detail = providerErrorDetail(error);
      toast.error(
        detail
          ? t(($) => $.weixin.connect_failed_detail, { details: detail })
          : t(($) => $.weixin.connect_failed),
      );
    }
  }
  if (isLoading) {
    return (
      <p className="text-body text-muted-foreground">
        {t(($) => $.weixin.loading)}
      </p>
    );
  }
  if (isError) {
    return (
      <p className="text-body text-muted-foreground">
        {t(($) => $.weixin.load_failed)}
      </p>
    );
  }
  if (!data?.configured) {
    return (
      <Card>
        <CardContent>
          <p className="text-body font-medium">
            {t(($) => $.weixin.not_enabled)}
          </p>
          <code className="text-micro">PATCHBAY_WEIXIN_SECRET_KEY</code>
        </CardContent>
      </Card>
    );
  }
  const installations = data.installations.filter(
    (installation) => !installationId || installation.id === installationId,
  );
  return installations.length ? (
    <Card>
      <CardContent className="divide-y">
        {installations.map((item) => (
          <div
            key={item.id}
            className="flex items-center justify-between py-3 first:pt-0 last:pb-0"
          >
            <div>
              <p className="text-body font-medium">{item.bot_id}</p>
              <p className="text-micro text-muted-foreground">
                {new Date(item.installed_at).toLocaleString()}
              </p>
            </div>
            {canManage && item.status === "active" ? (
              <Button
                variant="outline"
                size="sm"
                onClick={() => void disconnect(item.id)}
              >
                <Trash2 className="h-3 w-3" />
                {t(($) => $.weixin.disconnect)}
              </Button>
            ) : null}
          </div>
        ))}
      </CardContent>
    </Card>
  ) : (
    <Card>
      <CardContent>
        <p className="text-body text-muted-foreground">
          {t(($) => $.weixin.empty)}
        </p>
      </CardContent>
    </Card>
  );
}

// The settings Integrations card omits agentId so this action creates a
// workspace Hub. Agent-detail pages may still pass an id for legacy,
// explicitly bound installations.
export function WeixinAgentBindButton({ agentId }: { agentId?: string }) {
  const { t } = useT("settings");
  const wsId = useWorkspaceId();
  const qc = useQueryClient();
  const { data } = useQuery(weixinInstallationsOptions(wsId));
  const [open, setOpen] = useState(false);
  const [session, setSession] = useState<{
    id: string;
    qr: string;
    interval: number;
  } | null>(null);
  const [status, setStatus] = useState("pending");
  const [verifyCode, setVerifyCode] = useState("");
  const [errorMessage, setErrorMessage] = useState<string | null>(null);
  const existing = data?.installations.find(
    (item) =>
      (agentId ? item.agent_id === agentId : item.agent_id === null) &&
      item.status === "active",
  );

  useEffect(() => {
    if (!open || session) return;
    let cancelled = false;
    void api
      .beginWeixinInstall(wsId, agentId)
      .then((value) => {
        if (!cancelled) {
          setErrorMessage(null);
          setSession({
            id: value.session_id,
            qr: value.qr_code_content ?? value.qr_code_url,
            interval: value.poll_interval_seconds,
          });
        }
      })
      .catch((error: unknown) => {
        if (!cancelled) {
          const detail = providerErrorDetail(error);
          const message = detail
            ? t(($) => $.weixin.connect_failed_detail, { details: detail })
            : t(($) => $.weixin.connect_failed);
          setStatus("error");
          setErrorMessage(message);
          toast.error(message);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [agentId, open, session, t, wsId]);

  useEffect(() => {
    if (
      !open ||
      !session ||
      ["success", "expired", "error", "already_connected"].includes(status)
    ) {
      return;
    }
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const poll = async () => {
      try {
        const result = await api.getWeixinInstallStatus(
          wsId,
          session.id,
          verifyCode.trim(),
        );
        if (cancelled) return;
        setStatus(result.status);
        if (result.status === "success") {
          await qc.invalidateQueries({ queryKey: weixinKeys.installations(wsId) });
          toast.success(t(($) => $.weixin.connected));
          setOpen(false);
          return;
        }
      } catch (error: unknown) {
        if (!cancelled) {
          const detail = providerErrorDetail(error);
          setStatus("error");
          setErrorMessage(
            detail
              ? t(($) => $.weixin.connect_failed_detail, { details: detail })
              : t(($) => $.weixin.connect_failed),
          );
          return;
        }
      }
      if (!cancelled) {
        timer = setTimeout(
          () => void poll(),
          Math.max(2000, session.interval * 1000),
        );
      }
    };
    timer = setTimeout(
      () => void poll(),
      Math.max(2000, session.interval * 1000),
    );
    return () => {
      cancelled = true;
      if (timer) clearTimeout(timer);
    };
  }, [open, qc, session, status, t, verifyCode, wsId]);

  if (existing) {
    const healthy = isMessagingInstallationHealthy(existing);
    return (
      <div className="flex items-center gap-2">
        <span className={healthy ? "text-caption text-emerald-600" : "text-caption text-amber-600"}>
          {healthy
            ? t(($) => $.weixin.connected)
            : t(($) => $.page.integrations_status)}
        </span>
        <Button
          variant="outline"
          size="sm"
          onClick={async () => {
            try {
              await api.deleteWeixinInstallation(wsId, existing.id);
              await qc.invalidateQueries({
                queryKey: weixinKeys.installations(wsId),
              });
              toast.success(t(($) => $.weixin.disconnected));
            } catch (error) {
              const detail = providerErrorDetail(error);
              toast.error(
                detail
                  ? t(($) => $.weixin.connect_failed_detail, { details: detail })
                  : t(($) => $.weixin.connect_failed),
              );
            }
          }}
        >
          {t(($) => $.weixin.disconnect)}
        </Button>
      </div>
    );
  }
  if (!data?.install_supported) return null;
  const statusLabels: Record<string, string> = {
    pending: t(($) => $.weixin.status_pending),
    scanned: t(($) => $.weixin.status_scanned),
    need_verify_code: t(($) => $.weixin.status_need_verify_code),
    already_connected: t(($) => $.weixin.status_already_connected),
    expired: t(($) => $.weixin.status_expired),
    error: t(($) => $.weixin.status_error),
  };
  const statusLabel = statusLabels[status] ?? status;
  return (
    <>
      <Button
        variant="outline"
        size="sm"
        onClick={() => {
          setSession(null);
          setStatus("pending");
          setVerifyCode("");
          setErrorMessage(null);
          setOpen(true);
        }}
      >
        <WeixinMark className="h-4 w-4" />
        {t(($) => $.weixin.connect)}
      </Button>
      <Dialog open={open} onOpenChange={setOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{t(($) => $.weixin.scan_title)}</DialogTitle>
          </DialogHeader>
          <div className="flex flex-col items-center gap-4 py-4">
            {session ? (
              qrImageSource(session.qr) ? (
                <img
                  data-testid="weixin-qr-code"
                  data-value={session.qr}
                  src={qrImageSource(session.qr) ?? undefined}
                  alt={t(($) => $.weixin.scan_title)}
                  className="size-48 rounded-md bg-white p-2"
                />
              ) : (
                <span data-testid="weixin-qr-code" data-value={session.qr}>
                  <QRCode value={session.qr} size={192} />
                </span>
              )
            ) : (
              <p className="text-caption text-muted-foreground">
                {t(($) => $.weixin.starting)}
              </p>
            )}
            <p className="text-caption text-muted-foreground">
              {t(($) => $.weixin.status, { status: statusLabel })}
            </p>
            {errorMessage ? (
              <p className="text-caption text-destructive" role="alert">
                {errorMessage}
              </p>
            ) : null}
            {status === "need_verify_code" ? (
              <Input
                value={verifyCode}
                onChange={(event) => setVerifyCode(event.target.value)}
                placeholder={t(($) => $.weixin.verify_code)}
                className="max-w-48"
              />
            ) : null}
          </div>
        </DialogContent>
      </Dialog>
    </>
  );
}
