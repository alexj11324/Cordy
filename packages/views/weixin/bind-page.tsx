"use client";

import { useEffect, useState } from "react";
import { Card, CardContent } from "@patchbay/ui/components/ui/card";
import { Button } from "@patchbay/ui/components/ui/button";
import { api } from "@patchbay/core/api";
import { useAuthStore } from "@patchbay/core/auth";
import { useNavigation } from "../navigation";
import { useT } from "../i18n";

type State = "idle" | "redeeming" | "needs-auth" | "done" | "error";
export function WeixinBindPage({ token }: { token: string | null }) {
  const { t } = useT("common");
  const user = useAuthStore((s) => s.user);
  const loading = useAuthStore((s) => s.isLoading);
  const navigation = useNavigation();
  const [state, setState] = useState<State>("idle");
  useEffect(() => {
    if (!token) {
      setState("error");
      return;
    }
    if (loading) return;
    if (!user) {
      setState("needs-auth");
      return;
    }
    if (state !== "idle" && state !== "needs-auth") return;
    setState("redeeming");
    void api
      .redeemWeixinBindingToken(token)
      .then((result) =>
        setState(
          result.workspace_id && result.installation_id && result.weixin_user_id
            ? "done"
            : "error",
        ),
      )
      .catch(() => setState("error"));
  }, [loading, state, token, user]);
  return (
    <div className="mx-auto flex min-h-screen max-w-md items-center p-6">
      <Card className="w-full">
        <CardContent className="space-y-4">
          <h1 className="text-title font-semibold">{t(($) => $.weixin_bind.page_title)}</h1>
          {state === "done" ? (
            <p className="text-body">{t(($) => $.weixin_bind.done)}</p>
          ) : state === "error" ? (
            <p className="text-body text-destructive">{t(($) => $.weixin_bind.error)}</p>
          ) : state === "needs-auth" ? (
            <>
              <p className="text-body text-muted-foreground">
                {t(($) => $.weixin_bind.sign_in_description)}
              </p>
              <Button
                onClick={() =>
                  navigation.push(
                    `/login?next=${encodeURIComponent(
                      `/weixin/bind?token=${encodeURIComponent(token ?? "")}`,
                    )}`,
                  )
                }
              >
                {t(($) => $.weixin_bind.sign_in)}
              </Button>
            </>
          ) : (
            <p className="text-body text-muted-foreground">
              {t(($) => $.weixin_bind.redeeming)}
            </p>
          )}
        </CardContent>
      </Card>
    </div>
  );
}
