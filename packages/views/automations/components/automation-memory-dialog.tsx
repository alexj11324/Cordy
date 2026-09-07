"use client";

import { useEffect, useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import {
  automationMemoryListOptions,
  automationMemoryOptions,
  useDeleteAutomationMemory,
  useUpdateAutomationMemory,
} from "@patchbay/core/automations";
import { useWorkspaceId } from "@patchbay/core/hooks";
import type { AutomationMemoryFile, AutomationMemorySummary } from "@patchbay/core/types";
import { ApiError } from "@patchbay/core/api/client";
import { Button } from "@patchbay/ui/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@patchbay/ui/components/ui/dialog";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@patchbay/ui/components/ui/select";
import { Skeleton } from "@patchbay/ui/components/ui/skeleton";
import { Textarea } from "@patchbay/ui/components/ui/textarea";
import { toast } from "sonner";
import { useT } from "../../i18n";

const DEFAULT_MEMORY: AutomationMemorySummary = {
  name: "MEMORIES.md",
  revision: 0,
  updated_at: "",
};

interface MemoryDraft {
  content: string;
  revision: number;
  dirty: boolean;
}

function memoryFileFromSummary(summary: AutomationMemorySummary): AutomationMemoryFile {
  return { ...summary, content: "" };
}

export function AutomationMemoryDialog({
  automationId,
  onOpenChange,
}: {
  automationId: string;
  onOpenChange: (open: boolean) => void;
}) {
  const { t } = useT("automations");
  const wsId = useWorkspaceId();
  const listQuery = useQuery(automationMemoryListOptions(wsId, automationId));
  const updateMemory = useUpdateAutomationMemory();
  const deleteMemory = useDeleteAutomationMemory();
  const [selectedName, setSelectedName] = useState("");
  const [drafts, setDrafts] = useState<Record<string, MemoryDraft>>({});
  const [conflicts, setConflicts] = useState<Record<string, boolean>>({});
  const [recoveryFailures, setRecoveryFailures] = useState<Record<string, boolean>>({});
  const [recoveringName, setRecoveringName] = useState<string | null>(null);

  const items = useMemo(() => {
    if (!listQuery.isSuccess) return [];
    return listQuery.data.items.length > 0 ? listQuery.data.items : [DEFAULT_MEMORY];
  }, [listQuery.data, listQuery.isSuccess]);

  useEffect(() => {
    if (items.length === 0) return;
    if (items.some((item) => item.name === selectedName)) return;
    setSelectedName(items[0]?.name ?? "");
  }, [items, selectedName]);

  const selectedSummary = items.find((item) => item.name === selectedName);
  const isVirtual = selectedSummary?.revision === 0;
  const detailQuery = useQuery(
    automationMemoryOptions(wsId, automationId, selectedName, {
      enabled: Boolean(selectedSummary) && !isVirtual,
    }),
  );
  const authoritativeFile = isVirtual && selectedSummary
    ? memoryFileFromSummary(selectedSummary)
    : detailQuery.data?.name === selectedName
      ? detailQuery.data
      : undefined;

  useEffect(() => {
    if (!authoritativeFile) return;
    setDrafts((current) => {
      const existing = current[authoritativeFile.name];
      if (existing?.dirty) return current;
      if (
        existing?.content === authoritativeFile.content &&
        existing.revision === authoritativeFile.revision
      ) {
        return current;
      }
      return {
        ...current,
        [authoritativeFile.name]: {
          content: authoritativeFile.content,
          revision: authoritativeFile.revision,
          dirty: false,
        },
      };
    });
  }, [authoritativeFile]);

  const draft = selectedName ? drafts[selectedName] : undefined;
  const selectedLoading = Boolean(selectedSummary) && !isVirtual && detailQuery.isFetching;
  const selectedFailed = Boolean(selectedSummary) && !isVirtual && detailQuery.isError && !draft;
  const mutating = updateMemory.isPending || deleteMemory.isPending || recoveringName === selectedName;
  const hasConflict = Boolean(
    selectedName &&
    (conflicts[selectedName] || (
      draft && authoritativeFile && draft.revision !== authoritativeFile.revision
    )),
  );
  const recoveryFailed = Boolean(selectedName && recoveryFailures[selectedName]);
  const canSave = Boolean(
    selectedName &&
    draft?.dirty &&
    draft.revision === authoritativeFile?.revision &&
    !selectedLoading &&
    !listQuery.isFetching &&
    !selectedFailed &&
    !hasConflict &&
    !mutating,
  );

  const reset = () => {
    if (!authoritativeFile) return;
    setDrafts((current) => ({
      ...current,
      [authoritativeFile.name]: {
        content: authoritativeFile.content,
        revision: authoritativeFile.revision,
        dirty: false,
      },
    }));
    setConflicts((current) => ({ ...current, [authoritativeFile.name]: false }));
  };

  const recoverConflict = async (name: string) => {
    setConflicts((current) => ({ ...current, [name]: true }));
    setRecoveryFailures((current) => ({ ...current, [name]: false }));
    setRecoveringName(name);
    try {
      const refreshedList = await listQuery.refetch();
      if (refreshedList.isError || !refreshedList.data) {
        setRecoveryFailures((current) => ({ ...current, [name]: true }));
        return;
      }
      if (!refreshedList.data.items.some((item) => item.name === name)) return;
      const refreshedDetail = await detailQuery.refetch();
      if (refreshedDetail.isError || refreshedDetail.data?.name !== name) {
        setRecoveryFailures((current) => ({ ...current, [name]: true }));
      }
    } finally {
      setRecoveringName(null);
    }
  };

  const save = async () => {
    if (!canSave || !draft || !selectedSummary) return;
    const name = selectedName;
    try {
      const saved = await updateMemory.mutateAsync({
        automationId,
        name,
        content: draft.content,
        expected_revision: draft.revision,
      });
      if (saved.name !== name) throw new Error(t(($) => $.settings.tools_memories_invalid_response));
      setDrafts((current) => ({
        ...current,
        [name]: { content: saved.content, revision: saved.revision, dirty: false },
      }));
      setConflicts((current) => ({ ...current, [name]: false }));
      toast.success(t(($) => $.settings.tools_memories_saved));
    } catch (error) {
      if (error instanceof ApiError && error.status === 409) {
        await recoverConflict(name);
      }
      toast.error(
        error instanceof Error ? error.message : t(($) => $.settings.tools_memories_save_failed),
      );
    }
  };

  const remove = async () => {
    if (!authoritativeFile || authoritativeFile.revision === 0 || mutating) return;
    const name = authoritativeFile.name;
    try {
      await deleteMemory.mutateAsync({
        automationId,
        name,
        revision: authoritativeFile.revision,
      });
      setDrafts((current) => {
        const next = { ...current };
        delete next[name];
        return next;
      });
      setSelectedName("");
      toast.success(t(($) => $.settings.tools_memories_deleted));
    } catch (error) {
      if (error instanceof ApiError && error.status === 409) {
        await recoverConflict(name);
      }
      toast.error(
        error instanceof Error ? error.message : t(($) => $.settings.tools_memories_delete_failed),
      );
    }
  };

  return (
    <Dialog open onOpenChange={(open) => { if (!mutating) onOpenChange(open); }}>
      <DialogContent className="flex h-[min(680px,85vh)] grid-rows-none flex-col gap-4 sm:max-w-2xl">
        <DialogHeader>
          <DialogTitle>{t(($) => $.settings.tools_memories_dialog_title)}</DialogTitle>
          <DialogDescription>
            {t(($) => $.settings.tools_memories_dialog_description)}
          </DialogDescription>
        </DialogHeader>

        {listQuery.isPending ? (
          <div className="space-y-3" role="status" aria-label={t(($) => $.settings.tools_memories_loading)}>
            <Skeleton className="h-8 w-full" />
            <Skeleton className="h-80 w-full" />
          </div>
        ) : listQuery.isError ? (
          <div className="flex flex-1 flex-col items-center justify-center gap-3 text-center">
            <p role="alert" className="text-body text-destructive">
              {t(($) => $.settings.tools_memories_load_failed)}
            </p>
            <Button size="sm" variant="outline" onClick={() => void listQuery.refetch()}>
              {t(($) => $.page.retry)}
            </Button>
          </div>
        ) : (
          <div className="flex min-h-0 flex-1 flex-col gap-3">
            <label className="space-y-1">
              <span className="text-caption text-muted-foreground">
                {t(($) => $.settings.tools_memories_file)}
              </span>
              <Select
                value={selectedName}
                items={items.map((item) => ({ value: item.name, label: item.name }))}
                onValueChange={(name) => { if (name) setSelectedName(name); }}
              >
                <SelectTrigger
                  aria-label={t(($) => $.settings.tools_memories_file)}
                  className="w-full"
                  disabled={mutating}
                >
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {items.map((item) => (
                    <SelectItem key={item.name} value={item.name}>{item.name}</SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </label>

            {selectedLoading ? (
              <Skeleton className="min-h-0 flex-1" />
            ) : selectedFailed ? (
              <div className="flex flex-1 flex-col items-center justify-center gap-3 text-center">
                <p role="alert" className="text-body text-destructive">
                  {t(($) => $.settings.tools_memories_load_failed)}
                </p>
                <Button size="sm" variant="outline" onClick={() => void detailQuery.refetch()}>
                  {t(($) => $.page.retry)}
                </Button>
              </div>
            ) : (
              <div className="flex min-h-0 flex-1 flex-col gap-2">
                {hasConflict && (
                  <div className="flex items-center justify-between gap-3">
                    <p role="alert" className="text-caption text-amber-700 dark:text-amber-300">
                      {t(($) => $.settings.tools_memories_conflict)}
                    </p>
                    {recoveryFailed && (
                      <Button
                        size="sm"
                        variant="outline"
                        onClick={() => void recoverConflict(selectedName)}
                      >
                        {t(($) => $.page.retry)}
                      </Button>
                    )}
                  </div>
                )}
                <Textarea
                  aria-label={t(($) => $.settings.tools_memories_content)}
                  className="min-h-0 flex-1 resize-none font-mono text-body leading-6"
                  value={draft?.content ?? ""}
                  disabled={!draft || mutating}
                  onChange={(event) => {
                    const content = event.target.value;
                    setDrafts((current) => ({
                      ...current,
                      [selectedName]: {
                        content,
                        revision: current[selectedName]?.revision ?? selectedSummary?.revision ?? 0,
                        dirty: true,
                      },
                    }));
                  }}
                />
              </div>
            )}
          </div>
        )}

        <DialogFooter className="mt-auto">
          <Button
            size="sm"
            variant="ghost"
            className="mr-auto text-destructive hover:text-destructive"
            disabled={!authoritativeFile || authoritativeFile.revision === 0 || recoveryFailed || mutating}
            onClick={() => void remove()}
          >
            {t(($) => $.settings.tools_memories_delete)}
          </Button>
          <Button
            size="sm"
            variant="outline"
            disabled={!authoritativeFile || !draft?.dirty || recoveryFailed || mutating}
            onClick={reset}
          >
            {t(($) => $.settings.tools_memories_reset)}
          </Button>
          <Button size="sm" disabled={!canSave} onClick={() => void save()}>
            {updateMemory.isPending
              ? t(($) => $.settings.tools_memories_saving)
              : t(($) => $.settings.tools_memories_save)}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
