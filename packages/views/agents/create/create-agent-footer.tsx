"use client";

import { Loader2 } from "lucide-react";
import { Button } from "@orvilo/ui/components/ui/button";
import { useT } from "../../i18n";

/**
 * Where the flow ends. Commit and abandon sit together because they are the
 * two answers to one question; a discard parked elsewhere (a trash icon over
 * the transcript) reads as "clear this chat".
 *
 * This is a page action, not a dialog chrome bar: no top rule, no frosted
 * strip. The primary control sits in the center. `onDiscard` is optional —
 * the manual route has nothing to abandon.
 */
export function CreateAgentFooter({
  canCreate,
  creating,
  team,
  error,
  onCreate,
  onDiscard,
  discarding = false,
}: {
  canCreate: boolean;
  creating: boolean;
  team: boolean;
  error: string | null;
  onCreate: () => void;
  onDiscard?: () => void;
  discarding?: boolean;
}) {
  const { t } = useT("agents");
  return (
    <div className="flex flex-col items-center justify-center gap-3 px-5 py-6">
      {error ? (
        <p
          role="alert"
          className="max-w-lg text-center text-body text-destructive"
        >
          {error}
        </p>
      ) : null}
      <div className="flex items-center justify-center gap-3">
        {onDiscard ? (
          <Button
            type="button"
            variant="ghost"
            className="text-muted-foreground hover:text-foreground"
            onClick={onDiscard}
            disabled={creating || discarding}
          >
            {t(($) => $.creation_studio.drafts.discard)}
          </Button>
        ) : null}
        <Button
          type="button"
          className="shrink-0"
          onClick={onCreate}
          disabled={!canCreate}
        >
          {creating && <Loader2 className="size-4 animate-spin" />}
          {creating
            ? t(($) => $.creation_studio.creating)
            : team
              ? t(($) => $.creation_studio.create_and_add)
              : t(($) => $.creation_studio.create_and_open)}
        </Button>
      </div>
    </div>
  );
}
