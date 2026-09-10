"use client";

import { ExternalLink } from "lucide-react";
import { useConfigStore } from "@orvilo/core/config";
import { Button } from "@orvilo/ui/components/ui/button";
import { openExternal } from "../../platform";
import { useT } from "../../i18n";

export function useMessagingSetupWritable() {
  return useConfigStore((state) =>
    state.messaging?.mode === "managed" && state.messaging.setupWritable === true,
  );
}

export function MessagingSetupNotice() {
  const { t, i18n } = useT("settings");
  const mode = useConfigStore((state) => state.messaging?.mode);
  const serverConfigured = mode === "server_configured";
  const prefix = i18n.language.startsWith("zh") ? "/zh" : "";

  return (
    <div className="space-y-3" role="note">
      <p className="text-caption leading-5 text-muted-foreground">
        {serverConfigured
          ? t(($) => $.page.integrations_server_setup_description)
          : mode === "disabled"
            ? t(($) => $.page.integrations_disabled_description)
            : t(($) => $.page.integrations_hosted_setup_unavailable)}
      </p>
      {serverConfigured && (
        <Button
          type="button"
          variant="outline"
          size="sm"
          onClick={() => openExternal(`https://orvilo.aspectlylabs.com/docs${prefix}/self-host-messaging`)}
        >
          <ExternalLink className="size-3.5" />
          {t(($) => $.page.integrations_server_setup_docs)}
        </Button>
      )}
    </div>
  );
}
