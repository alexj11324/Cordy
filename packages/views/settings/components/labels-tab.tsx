"use client";

import { useEffect, useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import {
  ActionIcon,
  Button,
  DropdownMenu,
  Input,
  Modal,
  TextArea,
} from "@lobehub/ui/base-ui";
import { MoreHorizontal, Pencil, Plus, Tag, Trash2 } from "lucide-react";
import { toast } from "sonner";
import { useWorkspaceId } from "@orvilo/core/hooks";
import {
  labelListOptions,
  useCreateLabel,
  useDeleteLabel,
  useUpdateLabel,
} from "@orvilo/core/labels";
import type { Label, LabelResourceType } from "@orvilo/core/types";
import { ColorPicker, COLOR_PICKER_PRESETS } from "../../common/color-picker";
import { useLocale, useT } from "../../i18n";
import { SettingsEmptyState } from "./settings-empty";
import { SettingsFormRow, SettingsGroup } from "./settings-shell";
import { SettingsSearchBar } from "./settings-search";
import { useSettingsConfirm } from "./settings-confirm";

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

/**
 * Labels — the workspace's issue and skill catalogs, and the reference shape
 * for every other catalog tab.
 *
 * **Both of this tab's page-level controls used to be invisible inside the
 * settings dialog, and the shell's `extra` slot is where they render again.**
 * `SettingsTab` returns `children` and nothing else whenever it sits inside the
 * settings dialog (`settings-layout.tsx`), so the create-label button — the
 * tab's `action` — never rendered on the only surface this tab has. Moving it
 * to `SettingsGroup`'s `extra` fixes it, because `extra` renders wherever the
 * group sits. The scope pills and the search box were the old
 * `SettingsSection`'s `action`, which is the same slot by the mapping table, so
 * the tab has one header row carrying all three.
 *
 * The tab's own title and description are gone with `SettingsTab`. The title
 * belongs to the shell's `DialogHeader`; the description does not, because
 * `tabDescription("labels")` returns `""` — so it would have been dropped
 * rather than relocated, and it is the sentence that says what these two
 * catalogs *are*. It goes on the group, where it renders.
 *
 * **Every row is a `SettingsFormRow`, not a bespoke flex row.** That is the
 * shape `tokens-tab` set for a catalog whose rows carry an action: the row's
 * own `Form.Item` supplies the label, the description, and the antd geometry
 * `base.css` had to restate for these rows, and `divider` gives the card the
 * `divide-y` the old `SettingsCard` drew. No row passes `minWidth` — the
 * control column hugs its content (the usage figure, the date, and the overflow
 * menu) exactly as the old `SettingsListRow`'s did.
 *
 * **The delete confirmation is `useSettingsConfirm`, and the promise it returns
 * is load-bearing.** The old `AlertDialog` closed from the mutation's
 * `onSuccess`; the imperative modal closes on the line after `onOk` unless
 * `onOk` returns a promise, so `onConfirm` deliberately rethrows — a failed
 * delete leaves the dialog open on top of the toast rather than dismissing as
 * if the label were gone.
 */
export function LabelsTab() {
  const { t } = useT("settings");
  const locale = useLocale();
  const wsId = useWorkspaceId();
  const confirm = useSettingsConfirm();

  const [resourceType, setResourceType] = useState<LabelScope>("issue");
  const [query, setQuery] = useState("");
  const [editing, setEditing] = useState<Label | null>(null);
  const [createOpen, setCreateOpen] = useState(false);

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

  const remove = useDeleteLabel();

  const scopeLabel = t(($) => $.labels.scopes[resourceType]);

  /**
   * Rejects on failure on purpose: `useSettingsConfirm`'s `onOk` keeps the
   * dialog open only while its promise is unsettled, so swallowing the error
   * here would dismiss the confirmation exactly when the label still exists.
   */
  const openDeleteConfirm = (label: Label) => {
    confirm({
      title: t(($) => $.labels.delete_dialog.title),
      description: t(($) => $.labels.delete_dialog.description, {
        name: label.name,
        count: label.usage_count ?? 0,
      }),
      confirmLabel: t(($) => $.labels.delete_dialog.confirm),
      cancelLabel: t(($) => $.labels.delete_dialog.cancel),
      onConfirm: async () => {
        try {
          await remove.mutateAsync({
            id: label.id,
            resource_type: label.resource_type ?? "issue",
          });
        } catch (error) {
          toast.error(
            error instanceof Error
              ? error.message
              : t(($) => $.labels.delete_dialog.failed),
          );
          throw error;
        }
      },
    });
  };

  return (
    <>
      <SettingsGroup
        title={t(($) => $.labels.title)}
        description={t(($) => $.labels.description)}
        extra={
          <div className="flex flex-col items-end gap-2 sm:flex-row sm:flex-wrap sm:items-center">
            {/* The buttons keep their own row below `sm`. A single wrapping row
                is what the first version used, and it cost the group its title:
                a wrapping flex container's max-content contribution is the SUM
                of its items — the buttons plus the search's `w-full` — so at a
                460px viewport the group's `extra` claimed 336px of the 386px
                mobile header and squeezed the title to its min-content.
                Measured in the renderer: the title came out **17.6 x 44px, one
                character per line**; with this stack it is **28 x 22px, one
                line**, and the three buttons still share their own line. */}
            <div className="flex flex-wrap items-center justify-end gap-2">
              <Button
                icon={Plus}
                shape="round"
                type="primary"
                onClick={() => setCreateOpen(true)}
              >
                {t(($) => $.labels.new_label)}
              </Button>
              {RESOURCE_TYPES.map((type) => (
                <Button
                  key={type}
                  icon={Tag}
                  shape="round"
                  type={resourceType === type ? "primary" : "fill"}
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
                </Button>
              ))}
            </div>
            <SettingsSearchBar
              className="w-full sm:w-52"
              label={t(($) => $.labels.search_placeholder)}
              placeholder={t(($) => $.labels.search_placeholder)}
              value={query}
              onValueChange={setQuery}
            />
          </div>
        }
      >
        {isLoading ? (
          <SettingsEmptyState title={t(($) => $.labels.loading)} />
        ) : filteredLabels.length === 0 ? (
          <SettingsEmptyState
            title={
              query
                ? t(($) => $.labels.no_results)
                : t(($) => $.labels.empty, { scope: scopeLabel })
            }
          />
        ) : (
          filteredLabels.map((label, index) => (
            <SettingsFormRow
              key={label.id}
              divider={index > 0}
              label={
                <span className="inline-flex min-w-0 items-center gap-2">
                  <span
                    className="size-2.5 shrink-0 rounded-full"
                    style={{ backgroundColor: label.color }}
                  />
                  <span className="min-w-0 truncate">{label.name}</span>
                </span>
              }
              description={label.description || undefined}
            >
              <div className="flex shrink-0 items-center justify-end gap-3">
                <span className="hidden text-caption text-muted-foreground md:inline">
                  {t(($) => $.labels.usage_count, {
                    count: label.usage_count ?? 0,
                  })}
                </span>
                <span className="hidden text-caption text-muted-foreground md:inline">
                  {new Date(label.updated_at).toLocaleDateString(locale)}
                </span>
                <DropdownMenu
                  items={[
                    {
                      key: "edit",
                      icon: Pencil,
                      label: t(($) => $.labels.actions.edit),
                      onClick: () => setEditing(label),
                    },
                    {
                      key: "delete",
                      danger: true,
                      icon: Trash2,
                      label: t(($) => $.labels.actions.delete),
                      onClick: () => openDeleteConfirm(label),
                    },
                  ]}
                  placement="bottomRight"
                >
                  <ActionIcon
                    aria-label={t(($) => $.labels.actions.open, {
                      name: label.name,
                    })}
                    icon={MoreHorizontal}
                  />
                </DropdownMenu>
              </div>
            </SettingsFormRow>
          ))
        )}
      </SettingsGroup>

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
    </>
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

  /**
   * A self-contained dialog: its values exist only while it is open and no
   * server value can arrive late — the reference names that as the one place an
   * antd `Form` belongs. It stays a controlled draft anyway, unchanged from
   * before the migration: the reference's other condition is a *single* commit
   * point, and this dialog has two — it creates or updates depending on the row
   * it was opened from, and the payload differs between them.
   *
   * Seeding from the row is therefore still a `useEffect` on `[label, open]`.
   * `editing` is a snapshot of the row the menu was opened on, not a live query
   * value, so a refetch cannot reach in and overwrite typing.
   */
  const [current, setCurrent] = useState<LabelDraft>(EMPTY_DRAFT);

  useEffect(() => {
    if (!open) return;
    setCurrent(
      label
        ? {
            name: label.name,
            description: label.description ?? "",
            color: label.color,
          }
        : EMPTY_DRAFT,
    );
  }, [label, open]);

  const patchDraft = (patch: Partial<LabelDraft>) =>
    setCurrent((previous) => ({ ...previous, ...patch }));

  const saving = create.isPending || update.isPending;

  const submit = () => {
    const name = current.name.trim();
    if (!name) return;
    const payload = {
      name,
      description: current.description.trim(),
      color: current.color,
    };
    const onError = (error: unknown) =>
      toast.error(
        error instanceof Error ? error.message : t(($) => $.labels.save_failed),
      );
    if (label) {
      update.mutate(
        { id: label.id, resource_type: label.resource_type ?? resourceType, ...payload },
        { onSuccess: () => onOpenChange(false), onError },
      );
      return;
    }
    create.mutate(
      { resource_type: resourceType, ...payload },
      { onSuccess: () => onOpenChange(false), onError },
    );
  };

  return (
    <Modal
      destroyOnHidden
      open={open}
      title={
        label
          ? t(($) => $.labels.editor.edit_title)
          : t(($) => $.labels.editor.create_title)
      }
      width={480}
      footer={
        <div className="flex items-center justify-end gap-2">
          <Button shape="round" type="fill" onClick={() => onOpenChange(false)}>
            {t(($) => $.labels.editor.cancel)}
          </Button>
          <Button
            disabled={!current.name.trim() || saving}
            loading={saving}
            shape="round"
            type="primary"
            onClick={submit}
          >
            {saving
              ? t(($) => $.labels.editor.saving)
              : t(($) => $.labels.editor.save)}
          </Button>
        </div>
      }
      onCancel={() => onOpenChange(false)}
    >
      <div className="flex flex-col gap-5">
        <p className="text-body text-muted-foreground">
          {t(($) => $.labels.editor.scope_hint, {
            scope: t(($) => $.labels.scopes[resourceType]),
          })}
        </p>

        <div className="flex flex-col gap-1.5">
          <label className="text-body font-medium" htmlFor="label-name">
            {t(($) => $.labels.editor.name)}
          </label>
          <Input
            autoFocus
            id="label-name"
            maxLength={32}
            placeholder={t(($) => $.labels.editor.name_placeholder)}
            value={current.name}
            onChange={(event) => patchDraft({ name: event.target.value })}
          />
        </div>

        <div className="flex flex-col gap-1.5">
          <label className="text-body font-medium" htmlFor="label-description">
            {t(($) => $.labels.editor.description)}
          </label>
          <TextArea
            id="label-description"
            placeholder={t(($) => $.labels.editor.description_placeholder)}
            rows={3}
            value={current.description}
            onChange={(event) =>
              patchDraft({ description: event.target.value })
            }
          />
        </div>

        <div className="flex flex-col gap-1.5">
          <span className="text-body font-medium">
            {t(($) => $.labels.editor.color)}
          </span>
          <ColorPicker
            value={current.color}
            onChange={(color) => patchDraft({ color })}
            trigger={
              <button
                type="button"
                aria-label={t(($) => $.labels.editor.color)}
                className="flex h-9 items-center gap-2.5 rounded-md border border-surface-border px-2.5 transition-colors hover:bg-surface-hover"
              >
                <span
                  className="size-5 rounded-full"
                  style={{ backgroundColor: current.color }}
                />
                <span className="font-mono text-caption uppercase text-muted-foreground">
                  {current.color}
                </span>
              </button>
            }
          />
        </div>
      </div>
    </Modal>
  );
}
