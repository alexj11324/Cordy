"use client";

import { useState } from "react";
import { toast } from "sonner";
import { useWorkspaceId } from "@orvilo/core/hooks";
import { useCreateAutomationTrigger, useUpdateAutomationTrigger } from "@orvilo/core/automations/mutations";
import type { AutomationTrigger } from "@orvilo/core/types";
import { Button } from "@orvilo/ui/components/ui/button";
import { Dialog, DialogContent, DialogTitle } from "@orvilo/ui/components/ui/dialog";
import { browserTimezone } from "../../common/timezone-select";
import { useT } from "../../i18n";
import { ScheduleEditor } from "./schedule-editor/schedule-editor";
import { getDefaultScheduleConfig } from "./schedule-editor/model";
import { parseCron, toCron } from "./schedule-editor/cron-mapping";
import { useScheduleSubmitGate } from "./schedule-editor/validate";

// Mount only while open: a cancelled draft or rejected cron must not survive
// into the next edit. Both create and update use the same validation gate.
export function TriggerScheduleDialog({
  automationId,
  trigger,
  onOpenChange,
}: {
  automationId: string;
  trigger?: AutomationTrigger;
  onOpenChange: (open: boolean) => void;
}) {
  const { t } = useT("automations");
  const wsId = useWorkspaceId();
  const createTrigger = useCreateAutomationTrigger();
  const updateTrigger = useUpdateAutomationTrigger();
  const [config, setConfig] = useState(() => trigger?.cron_expression
    ? parseCron(trigger.cron_expression, trigger.timezone ?? "UTC")
    : getDefaultScheduleConfig(browserTimezone()));
  const [submitting, setSubmitting] = useState(false);
  const gate = useScheduleSubmitGate(wsId);

  const submit = async () => {
    if (submitting || !gate.scheduleValid) return;
    setSubmitting(true);
    try {
      if (!(await gate.ensureAccepted(config))) return;
      const schedule = { automationId, cron_expression: toCron(config), timezone: config.timezone };
      if (trigger) await updateTrigger.mutateAsync({ ...schedule, triggerId: trigger.id });
      else await createTrigger.mutateAsync({ ...schedule, kind: "schedule" });
      toast.success(trigger
        ? t(($) => $.settings.toast_trigger_updated)
        : t(($) => $.add_trigger_dialog.toast_added_schedule));
      onOpenChange(false);
    } catch (error) {
      toast.error(error instanceof Error ? error.message : t(($) => $.dialog.toast_update_failed));
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <Dialog open onOpenChange={(open) => { if (!submitting) onOpenChange(open); }}>
      <DialogContent className="max-w-md">
        <DialogTitle>{trigger ? t(($) => $.settings.edit_schedule) : t(($) => $.presets.scheduled)}</DialogTitle>
        <div className="min-w-0 space-y-4 pt-2">
          <ScheduleEditor
            value={config}
            onChange={(next) => { gate.clearRejection(); setConfig(next); }}
            wsId={wsId}
            onValidityChange={gate.onValidityChange}
            disabled={submitting}
          />
          <div className="flex justify-end gap-2 pt-1">
            <Button size="sm" variant="outline" disabled={submitting} onClick={() => onOpenChange(false)}>
              {t(($) => $.detail.delete_dialog.cancel)}
            </Button>
            <Button size="sm" disabled={submitting || !gate.scheduleValid} onClick={submit}>
              {submitting ? t(($) => $.dialog.saving)
                : trigger ? t(($) => $.dialog.save) : t(($) => $.add_trigger_dialog.submit)}
            </Button>
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}
