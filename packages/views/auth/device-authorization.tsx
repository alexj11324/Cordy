"use client";

import { useState } from "react";
import { api } from "@orvilo/core/api";
import { Button } from "@orvilo/ui/components/ui/button";
import { Input } from "@orvilo/ui/components/ui/input";
import { useT } from "../i18n";

type PendingRequest = {
  code: string;
  clientName: string;
};

export function DeviceAuthorization() {
  const { t } = useT("auth");
  const [code, setCode] = useState("");
  const [request, setRequest] = useState<PendingRequest | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [decision, setDecision] = useState<"approved" | "denied" | null>(null);

  const inspect = async () => {
    const userCode = code.trim().toUpperCase();
    setBusy(true);
    setError("");
    try {
      const result = await api.inspectDeviceAuthorization(userCode);
      setRequest({ code: userCode, clientName: result.client_name });
    } catch {
      setError(t(($) => $.device.invalid_code));
    } finally {
      setBusy(false);
    }
  };

  const decide = async (approve: boolean) => {
    if (!request) return;
    setBusy(true);
    setError("");
    try {
      await api.decideDeviceAuthorization(request.code, approve);
      setDecision(approve ? "approved" : "denied");
    } catch {
      setError(t(($) => $.device.decision_failed));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="w-full max-w-sm space-y-5">
      <h1 className="text-display-sm font-semibold">
        {t(($) => $.device.title)}
      </h1>
      {decision ? (
        <p role="status">
          {decision === "approved"
            ? t(($) => $.device.approved)
            : t(($) => $.device.denied)}
        </p>
      ) : request ? (
        <>
          <p>{t(($) => $.device.confirm, { name: request.clientName })}</p>
          <p className="text-body text-muted-foreground">
            {t(($) => $.device.scope)}
          </p>
          <code className="block text-title tracking-widest">
            {request.code}
          </code>
          <div className="flex gap-2">
            <Button disabled={busy} onClick={() => void decide(true)}>
              {t(($) => $.device.approve)}
            </Button>
            <Button
              variant="outline"
              disabled={busy}
              onClick={() => void decide(false)}
            >
              {t(($) => $.device.deny)}
            </Button>
          </div>
        </>
      ) : (
        <form
          className="space-y-4"
          onSubmit={(event) => {
            event.preventDefault();
            void inspect();
          }}
        >
          <label className="block space-y-2">
            <span>{t(($) => $.device.code_label)}</span>
            <Input
              value={code}
              onChange={(event) => setCode(event.target.value.toUpperCase())}
              autoComplete="one-time-code"
              autoCapitalize="characters"
              spellCheck={false}
              maxLength={16}
              disabled={busy}
              required
            />
          </label>
          <p className="text-body text-muted-foreground">
            {t(($) => $.device.code_hint)}
          </p>
          <Button type="submit" disabled={busy || !code.trim()}>
            {t(($) => $.device.continue)}
          </Button>
        </form>
      )}
      {error ? (
        <p role="alert" className="text-body text-destructive">
          {error}
        </p>
      ) : null}
    </div>
  );
}
