"use client";

import { useEffect, useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { MoreHorizontal, Pencil, Plus, Tag, Trash2 } from "lucide-react";
import { toast } from "sonner";
import { useWorkspaceId } from "@patchbay/core/hooks";
import {
  labelListOptions,
  useCreateLabel,
  useDeleteLabel,
  useUpdateLabel,
} from "@patchbay/core/labels";
import type { Label, LabelResourceType } from "@patchbay/core/types";
import { Button } from "@patchbay/ui/components/ui/button";
import { Input } from "@patchbay/ui/components/ui/input";
import { Textarea } from "@patchbay/ui/components/ui/textarea";
import { Label as FieldLabel } from "@patchbay/ui/components/ui/label";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@patchbay/ui/components/ui/dialog";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@patchbay/ui/components/ui/alert-dialog";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@patchbay/ui/components/ui/dropdown-menu";
import { ColorPicker, COLOR_PICKER_PRESETS } from "../../common/color-picker";
import { useLocale, useT } from "../../i18n";
import { SettingsCard, SettingsEmpty, SettingsIconButton, SettingsListRow, SettingsPillButton, SettingsSearchField, SettingsSection, SettingsTab } from "./settings-layout";

/**
 * Label scopes this settings tab manages. Narrower than `LabelResourceType`:
 * the backend still models agent labels, but the product no longer exposes
 * any way to create, apply, or view them, so they are not manageable here.
 */
type LabelScope = Extract<LabelResourceType, "issue" | "skill">;

const RESOURCE_TYPES: LabelScope[] = ["issue", "skill"];

interface LabelDraft {
  name: string;
  description: string;
  color: string;
}

const EMPTY_DRAFT: LabelDraft = {
  name: "",
  description: "",
  color: COLOR_PICKER_PRESETS[6],
};

export function LabelsTab() {
  const { t } = useT("settings");
  const locale = useLocale();
  const wsId = useWorkspaceId();

  const [resourceType, setResourceType] = useState<LabelScope>("issue");
  const [query, setQuery] = useState("");
  const [editing, setEditing] = useState<Label | null>(null);
  const [createOpen, setCreateOpen] = useState(false);
  const [pendingDelete, setPendingDelete] = useState<Label | null>(null);

  const { data: labels = [], isLoading } = useQuery(
    labelListOptions(wsId, resourceType),
  );
  const filteredLabels = useMemo(() => {
    const normalized = query.trim().toLowerCase();
    if (!normalized) return labels;
    return labels.filter(
      (label) =>
        label.name.toLowerCase().includes(normalized) ||
        (label.description ?? "").toLowerCase().includes(normalized),
    );
  }, [labels, query]);

  const scopeLabel = t(($) => $.labels.scopes[resourceType]);

  return (
    <SettingsTab
      title={t(($) => $.labels.title)}
      description={t(($) => $.labels.description)}
      action={
        <SettingsPillButton icon={Plus} active onClick={() => setCreateOpen(true)}>
          {t(($) => $.labels.new_label)}
        </SettingsPillButton>
      }
    >
      <SettingsSection
        action={
          <div className="flex flex-wrap items-center justify-end gap-2">
            {RESOURCE_TYPES.map((type) => (
              <SettingsPillButton
                key={type}
                icon={Tag}
                active={resourceType === type}
                onClick={() => {
                  setResourceType(type);
                  setQuery("");
                }}
              >
                {t(($) => $.labels.scopes[type])}
                {type === resourceType ? (
                  <span className="tabular-nums opacity-80">
                    {labels.length}
                  </span>
                ) : null}
              </SettingsPillButton>
            ))}
            <SettingsSearchField
              value={query}
              onValueChange={setQuery}
              placeholder={t(($) => $.labels.search_placeholder)}
              className="w-full sm:w-52"
            />
          </div>
        }
      >
        <SettingsCard>
          {isLoading ? (
            <SettingsEmpty title={t(($) => $.labels.loading)} />
          ) : filteredLabels.length === 0 ? (
            <SettingsEmpty
              title={
                query
                  ? t(($) => $.labels.no_results)
                  : t(($) => $.labels.empty, { scope: scopeLabel })
              }
            />
          ) : (
            filteredLabels.map((label) => (
              <SettingsListRow key={label.id}>
                <span
                  className="size-2.5 shrink-0 rounded-full"
                  style={{ backgroundColor: label.color }}
                />
                <div className="min-w-0 flex-1">
                  <div className="truncate text-body font-medium">{label.name}</div>
                  {label.description ? (
                    <p className="truncate text-body text-muted-foreground">
                      {label.description}
                    </p>
                  ) : null}
                </div>
                <span className="hidden shrink-0 text-caption text-muted-foreground md:inline">
                  {t(($) => $.labels.usage_count, { count: label.usage_count ?? 0 })}
                </span>
                <span className="hidden shrink-0 text-caption text-muted-foreground md:inline">
                  {new Date(label.updated_at).toLocaleDateString(locale)}
                </span>
                <DropdownMenu>
                  <DropdownMenuTrigger
                    render={
                      <SettingsIconButton
                        aria-label={t(($) => $.labels.actions.open, { name: label.name })}
                      >
                        <MoreHorizontal className="size-4" />
                      </SettingsIconButton>
                    }
                  />
                  <DropdownMenuContent align="end">
                    <DropdownMenuItem onClick={() => setEditing(label)}>
                      <Pencil className="size-4" />
                      {t(($) => $.labels.actions.edit)}
                    </DropdownMenuItem>
                    <DropdownMenuItem
                      variant="destructive"
                      onClick={() => setPendingDelete(label)}
                    >
                      <Trash2 className="size-4" />
                      {t(($) => $.labels.actions.delete)}
                    </DropdownMenuItem>
                  </DropdownMenuContent>
                </DropdownMenu>
              </SettingsListRow>
            ))
          )}
        </SettingsCard>
      </SettingsSection>

      <LabelEditorDialog
        open={createOpen}
        onOpenChange={setCreateOpen}
        resourceType={resourceType}
      />
      <LabelEditorDialog
        open={Boolean(editing)}
        onOpenChange={(open) => !open && setEditing(null)}
        resourceType={resourceType}
        label={editing}
      />
      <DeleteLabelDialog
        label={pendingDelete}
        onClose={() => setPendingDelete(null)}
      />
    </SettingsTab>
  );
}

function LabelEditorDialog({
  open,
  onOpenChange,
  resourceType,
  label,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  resourceType: LabelScope;
  label?: Label | null;
}) {
  const { t } = useT("settings");
  const create = useCreateLabel();
  const update = useUpdateLabel();
  const [draft, setDraft] = useState<LabelDraft>(EMPTY_DRAFT);

  useEffect(() => {
    if (!open) return;
    setDraft(
      label
        ? {
            name: label.name,
            description: label.description ?? "",
            color: label.color,
          }
        : EMPTY_DRAFT,
    );
  }, [label, open]);

  const submit = () => {
    const name = draft.name.trim();
    if (!name) return;
    if (label) {
      update.mutate(
        {
          id: label.id,
          resource_type: label.resource_type ?? resourceType,
          name,
          description: draft.description.trim(),
          color: draft.color,
        },
        {
          onSuccess: () => onOpenChange(false),
          onError: (error) =>
            toast.error(error instanceof Error ? error.message : t(($) => $.labels.save_failed)),
        },
      );
      return;
    }
    create.mutate(
      {
        resource_type: resourceType,
        name,
        description: draft.description.trim(),
        color: draft.color,
      },
      {
        onSuccess: () => onOpenChange(false),
        onError: (error) =>
          toast.error(error instanceof Error ? error.message : t(($) => $.labels.save_failed)),
      },
    );
  };

  return (
    <Dialog
      open={open}
      onOpenChange={onOpenChange}
    >
      <DialogContent className="sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>
            {label ? t(($) => $.labels.editor.edit_title) : t(($) => $.labels.editor.create_title)}
          </DialogTitle>
          <DialogDescription>
            {t(($) => $.labels.editor.scope_hint, {
              scope: t(($) => $.labels.scopes[resourceType]),
            })}
          </DialogDescription>
        </DialogHeader>
        <div className="space-y-5 py-2">
          <div className="space-y-2">
            <FieldLabel htmlFor="label-name">{t(($) => $.labels.editor.name)}</FieldLabel>
            <Input
              id="label-name"
              autoFocus
              maxLength={32}
              value={draft.name}
              onChange={(event) => setDraft((current) => ({ ...current, name: event.target.value }))}
              placeholder={t(($) => $.labels.editor.name_placeholder)}
            />
          </div>
          <div className="space-y-2">
            <FieldLabel htmlFor="label-description">
              {t(($) => $.labels.editor.description)}
            </FieldLabel>
            <Textarea
              id="label-description"
              rows={3}
              value={draft.description}
              onChange={(event) =>
                setDraft((current) => ({ ...current, description: event.target.value }))
              }
              placeholder={t(($) => $.labels.editor.description_placeholder)}
            />
          </div>
          <div className="space-y-2">
            <FieldLabel>{t(($) => $.labels.editor.color)}</FieldLabel>
            <ColorPicker
              value={draft.color}
              onChange={(color) => setDraft((current) => ({ ...current, color }))}
              trigger={
                <button
                  type="button"
                  aria-label={t(($) => $.labels.editor.color)}
                  className="flex h-9 items-center gap-2.5 rounded-md border border-surface-border px-2.5 transition-colors hover:bg-surface-hover"
                >
                  <span
                    className="size-5 rounded-full"
                    style={{ backgroundColor: draft.color }}
                  />
                  <span className="font-mono text-caption uppercase text-muted-foreground">
                    {draft.color}
                  </span>
                </button>
              }
            />
          </div>
        </div>
        <DialogFooter>
          <Button variant="ghost" onClick={() => onOpenChange(false)}>
            {t(($) => $.labels.editor.cancel)}
          </Button>
          <Button
            onClick={submit}
            disabled={!draft.name.trim() || create.isPending || update.isPending}
          >
            {create.isPending || update.isPending
              ? t(($) => $.labels.editor.saving)
              : t(($) => $.labels.editor.save)}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

function DeleteLabelDialog({
  label,
  onClose,
}: {
  label: Label | null;
  onClose: () => void;
}) {
  const { t } = useT("settings");
  const remove = useDeleteLabel();
  return (
    <AlertDialog open={Boolean(label)} onOpenChange={(open) => !open && onClose()}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>{t(($) => $.labels.delete_dialog.title)}</AlertDialogTitle>
          <AlertDialogDescription>
            {t(($) => $.labels.delete_dialog.description, {
              name: label?.name ?? "",
              count: label?.usage_count ?? 0,
            })}
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>{t(($) => $.labels.delete_dialog.cancel)}</AlertDialogCancel>
          <AlertDialogAction
            onClick={() => {
              if (!label) return;
              remove.mutate(
                { id: label.id, resource_type: label.resource_type ?? "issue" },
                {
                  onSuccess: onClose,
                  onError: (error) =>
                    toast.error(
                      error instanceof Error
                        ? error.message
                        : t(($) => $.labels.delete_dialog.failed),
                    ),
                },
              );
            }}
          >
            {t(($) => $.labels.delete_dialog.confirm)}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
