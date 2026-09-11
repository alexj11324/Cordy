"use client";

import { useEffect, useMemo, useState } from "react";
import { toast } from "sonner";
import { useQuery } from "@tanstack/react-query";
import { ApiError } from "@orvilo/core/api";
import {
  useDeleteRuntime,
  useUnbindAgentsAndDeleteRuntime,
} from "@orvilo/core/runtimes/mutations";
import { agentListOptions } from "@orvilo/core/workspace/queries";
import {
  AlertDialog,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@orvilo/ui/components/ui/alert-dialog";
import { Button } from "@orvilo/ui/components/ui/button";
import { Checkbox } from "@orvilo/ui/components/ui/checkbox";
import { useT } from "../../i18n";
import { isPendingCustomRuntime } from "./pending-runtime";
import type { RuntimeMachine } from "./runtime-machines";
import { isSelfHealingRuntime } from "../utils";

export interface DeleteMachineDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  machine: RuntimeMachine;
  wsId: string;
  onStopLocalDaemon?: () => Promise<void>;
  onDeleted: () => void;
}

function isProfileInstanceRefusal(error: unknown): boolean {
  return (
    error instanceof ApiError &&
    error.status === 409 &&
    typeof error.body === "object" &&
    error.body !== null &&
    "code" in error.body &&
    (error.body as { code?: unknown }).code ===
      "runtime_profile_instance_delete_unsupported"
  );
}

export function canDeleteRuntimeMachine(
  machine: RuntimeMachine | null | undefined,
  opts: { isAdmin: boolean; currentUserId?: string | null },
): boolean {
  if (!machine) return false;
  const deletable = machine.runtimes.filter(
    (runtime) => !isPendingCustomRuntime(runtime),
  );
  if (deletable.length === 0) return false;
  if (opts.isAdmin) return true;
  return deletable.some((runtime) => runtime.owner_id === opts.currentUserId);
}

export function DeleteMachineDialog({
  open,
  onOpenChange,
  machine,
  wsId,
  onStopLocalDaemon,
  onDeleted,
}: DeleteMachineDialogProps) {
  const { t } = useT("runtimes");
  const { data: agents = [] } = useQuery(agentListOptions(wsId));
  const deleteRuntime = useDeleteRuntime(wsId);
  const unbindAndDelete = useUnbindAgentsAndDeleteRuntime(wsId);
  const [confirmed, setConfirmed] = useState(false);
  const [submitting, setSubmitting] = useState(false);

  const deletableRuntimes = useMemo(
    () => machine.runtimes.filter((runtime) => !isPendingCustomRuntime(runtime)),
    [machine.runtimes],
  );
  const boundAgents = useMemo(
    () =>
      agents.filter(
        (agent) =>
          !agent.archived_at &&
          deletableRuntimes.some((runtime) => runtime.id === agent.runtime_id),
      ),
    [agents, deletableRuntimes],
  );
  const selfHealing = deletableRuntimes.some((runtime) =>
    isSelfHealingRuntime(runtime),
  );
  const willStopDaemon = Boolean(machine.isCurrent && onStopLocalDaemon);

  useEffect(() => {
    if (open) {
      setConfirmed(false);
      setSubmitting(false);
    }
  }, [open]);

  async function handleConfirm() {
    if (!confirmed) return;
    if (deletableRuntimes.length === 0) {
      toast.error(t(($) => $.machine.delete_dialog.toast_failed));
      return;
    }
    setSubmitting(true);
    try {
      if (willStopDaemon && onStopLocalDaemon) {
        await onStopLocalDaemon();
      }
      for (const runtime of deletableRuntimes) {
        const boundIds = boundAgents
          .filter((agent) => agent.runtime_id === runtime.id)
          .map((agent) => agent.id);
        try {
          if (boundIds.length > 0) {
            await unbindAndDelete.mutateAsync({
              runtimeId: runtime.id,
              expectedActiveAgentIds: boundIds,
            });
          } else {
            await deleteRuntime.mutateAsync(runtime.id);
          }
        } catch (error) {
          if (!isProfileInstanceRefusal(error)) throw error;
        }
      }
      toast.success(t(($) => $.machine.delete_dialog.toast_deleted));
      onDeleted();
      onOpenChange(false);
    } catch {
      toast.error(t(($) => $.machine.delete_dialog.toast_failed));
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <AlertDialog open={open} onOpenChange={onOpenChange}>
      <AlertDialogContent className="w-[calc(100vw-2rem)] !max-w-[440px]">
        <AlertDialogHeader>
          <AlertDialogTitle>
            {t(($) => $.machine.delete_dialog.title, { name: machine.title })}
          </AlertDialogTitle>
          <AlertDialogDescription>
            {t(($) => $.machine.delete_dialog.description)}
          </AlertDialogDescription>
        </AlertDialogHeader>
        {selfHealing && !willStopDaemon ? (
          <p className="text-caption text-warning">
            {t(($) => $.machine.delete_dialog.self_heal_notice)}
          </p>
        ) : null}
        {willStopDaemon ? (
          <p className="text-caption text-muted-foreground">
            {t(($) => $.machine.delete_dialog.stop_daemon_notice)}
          </p>
        ) : null}
        <label className="flex items-start gap-2 text-caption">
          <Checkbox
            checked={confirmed}
            onCheckedChange={(value) => setConfirmed(value === true)}
          />
          <span>{t(($) => $.machine.delete_dialog.confirm)}</span>
        </label>
        <AlertDialogFooter>
          <Button
            type="button"
            variant="outline"
            disabled={submitting}
            onClick={() => onOpenChange(false)}
          >
            {t(($) => $.machine.delete_dialog.cancel)}
          </Button>
          <Button
            type="button"
            variant="destructive"
            disabled={!confirmed || submitting}
            onClick={() => void handleConfirm()}
          >
            {submitting
              ? t(($) => $.machine.delete_dialog.submitting)
              : t(($) => $.machine.delete_dialog.submit)}
          </Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
