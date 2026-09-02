"use client";

import { useEffect, useState } from "react";
import { Card, CardContent } from "@patchbay/ui/components/ui/card";
import { Button } from "@patchbay/ui/components/ui/button";
import { api } from "@patchbay/core/api";
import { useAuthStore } from "@patchbay/core/auth";
import { AppLink } from "../navigation";
import { useT } from "../i18n";

type RedeemState =
  | { kind: "idle" }
  | { kind: "redeeming" }
  | { kind: "done" }
  | { kind: "needs-auth" }
  | { kind: "error"; reason: string };

export function WeixinBindPage({ token }: { token: string | null }) {
  const { t } = useT("common");
  const user = useAuthStore((s) => s.user);
  const isAuthLoading = useAuthStore((s) => s.isLoading);
  const [state, setState] = useState<RedeemState>({ kind: "idle" });

  useEffect(() => {
    if (!token) {
      setState({ kind: "error", reason: "missing_token" });
      return;
    }
    if (isAuthLoading) return;
    if (!user) {
      setState({ kind: "needs-auth" });
      return;
    }
    if (state.kind !== "idle" && state.kind !== "needs-auth") return;
    setState({ kind: "redeeming" });
    void (async () => {
      try {
        await api.redeemWeixinBindingToken(token);
        setState({ kind: "done" });
      } catch (error) {
        setState({ kind: "error", reason: redemptionFailureReason(error) });
      }
    })();
  }, [token, user, isAuthLoading, state.kind]);

  return (
    <div className="mx-auto flex min-h-screen max-w-md flex-col items-center justify-center p-6">
      <Card className="w-full">
        <CardContent className="space-y-4">
          <h1 className="text-title font-semibold">{t(($) => $.weixin_bind.page_title)}</h1>
          {state.kind === "idle" || state.kind === "redeeming" ? (
            <p className="text-body text-muted-foreground" aria-live="polite">
              {t(($) => $.weixin_bind.redeeming)}
            </p>
          ) : state.kind === "needs-auth" ? (
            <>
              <p className="text-body text-muted-foreground">{t(($) => $.weixin_bind.needs_auth_description)}</p>
              <Button
                size="sm"
                render={
                  <AppLink
                    href={`/login?next=${encodeURIComponent(
                      `/weixin/bind?token=${encodeURIComponent(token ?? "")}`,
                    )}`}
                  />
                }
                nativeButton={false}
              >
                {t(($) => $.weixin_bind.sign_in)}
              </Button>
            </>
          ) : state.kind === "done" ? (
            <>
              <p className="text-body font-medium">{t(($) => $.weixin_bind.done_title)}</p>
              <p className="text-caption text-muted-foreground">{t(($) => $.weixin_bind.done_description)}</p>
            </>
          ) : (
            <>
              <p className="text-body font-medium">{t(($) => $.weixin_bind.error_title)}</p>
              <p className="text-caption text-muted-foreground">
                {state.reason === "missing_token"
                  ? t(($) => $.weixin_bind.error_missing_token)
                  : state.reason === "expired"
                    ? t(($) => $.weixin_bind.error_expired)
                    : state.reason === "already_bound"
                      ? t(($) => $.weixin_bind.error_already_bound)
                      : state.reason === "not_member"
                        ? t(($) => $.weixin_bind.error_not_member)
                        : t(($) => $.weixin_bind.error_unknown)}
              </p>
              <p className="text-micro text-muted-foreground">{t(($) => $.weixin_bind.error_admin_hint)}</p>
            </>
          )}
        </CardContent>
      </Card>
    </div>
  );
}

function redemptionFailureReason(error: unknown): string {
  const message = error instanceof Error ? error.message.toLowerCase() : "";
  if (message.includes("invalid") || message.includes("expired") || message.includes("410")) return "expired";
  if (message.includes("already bound") || message.includes("409")) return "already_bound";
  if (message.includes("workspace member") || message.includes("403")) return "not_member";
  return "unknown";
}
