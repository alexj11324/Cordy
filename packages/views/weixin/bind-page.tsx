"use client";

import { useEffect, useRef, useState } from "react";
import { Card, CardContent } from "@patchbay/ui/components/ui/card";
import { Button } from "@patchbay/ui/components/ui/button";
import { ApiError, api } from "@patchbay/core/api";
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
  const redeemedTokenRef = useRef<string | null>(null);

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
    if (redeemedTokenRef.current === token) return;
    redeemedTokenRef.current = token;
    setState({ kind: "redeeming" });
    void (async () => {
      try {
        const response = await api.redeemWeixinBindingToken(token);
        if (!response.workspace_id || !response.installation_id || !response.weixin_user_id) {
          throw new Error("Weixin binding returned a malformed response");
        }
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
  if (!(error instanceof ApiError)) return "unknown";
  if (error.status === 410) return "expired";
  if (error.status === 409) return "already_bound";
  if (error.status === 403) return "not_member";
  return "unknown";
}
