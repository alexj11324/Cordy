"use client";

import { useEffect, useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import {
  Archive,
  ArchiveRestore,
  GripVertical,
  MoreHorizontal,
  Pencil,
  Plus,
  SlidersHorizontal,
  X,
} from "lucide-react";
import { toast } from "sonner";
import { useAuthStore } from "@patchbay/core/auth";
import { useWorkspaceId } from "@patchbay/core/hooks";
import { memberListOptions } from "@patchbay/core/workspace/queries";
import {
  propertyListOptions,
  useCreateProperty,
  useUpdateProperty,
} from "@patchbay/core/properties";
import type {
  IssueProperty,
  IssuePropertyOption,
  IssuePropertyType,
} from "@patchbay/core/types";
import { ISSUE_PROPERTY_TYPES } from "@patchbay/core/types";
import { Button } from "@patchbay/ui/components/ui/button";
import { Badge } from "@patchbay/ui/components/ui/badge";
import { Input } from "@patchbay/ui/components/ui/input";
import { Textarea } from "@patchbay/ui/components/ui/textarea";
import { Label as FieldLabel } from "@patchbay/ui/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@patchbay/ui/components/ui/select";
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
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@patchbay/ui/components/ui/popover";
import { ColorPicker, COLOR_PICKER_PRESETS } from "../../common/color-picker";
import {
  PropertyIcon,
  PropertyIconGlyph,
  PropertyIconPicker,
} from "../../common/property-icon";
import { useT } from "../../i18n";
import {
  SettingsCard,
  SettingsEmpty,
  SettingsIconButton,
  SettingsListRow,
  SettingsPillButton,
  SettingsSearchField,
  SettingsSection,
  SettingsTab,
} from "./settings-layout";

const MAX_ACTIVE_PROPERTIES = 20;

interface OptionDraft {
  id?: string;
  name: string;
  color: string;
}

interface PropertyDraft {
  name: string;
  type: IssuePropertyType;
  description: string;
  icon: string;
  options: OptionDraft[];
}

const EMPTY_DRAFT: PropertyDraft = {
  name: "",
  type: "select",
  description: "",
  icon: "",
  options: [{ name: "", color: COLOR_PICKER_PRESETS[6] }],
};

function typeHasOptions(type: string): boolean {
  return type === "select" || type === "multi_select";
}

export function PropertiesTab() {
  const { t } = useT("settings");
  const wsId = useWorkspaceId();
  const user = useAuthStore((s) => s.user);

  const [query, setQuery] = useState("");
  const [showArchived, setShowArchived] = useState(false);
  const [editing, setEditing] = useState<IssueProperty | null>(null);
  const [createOpen, setCreateOpen] = useState(false);
  const [pendingArchive, setPendingArchive] = useState<IssueProperty | null>(null);

  const { data: members = [] } = useQuery(memberListOptions(wsId));
  const currentMember = members.find((m) => m.user_id === user?.id) ?? null;
  const canManage = currentMember?.role === "owner" || currentMember?.role === "admin";

  const { data: properties = [], isLoading } = useQuery(propertyListOptions(wsId, true));
  const update = useUpdateProperty();

  const activeCount = useMemo(
    () => properties.filter((p) => !p.archived).length,
    [properties],
  );
  const visible = useMemo(() => {
    const normalized = query.trim().toLowerCase();
    return properties
      .filter((p) => (showArchived ? true : !p.archived))
      .filter((p) => !normalized || p.name.toLowerCase().includes(normalized));
  }, [properties, query, showArchived]);

  return (
    <SettingsTab
      title={t(($) => $.properties.title)}
      description={t(($) => $.properties.description)}
      action={
        canManage ? (
          <SettingsPillButton
            icon={Plus}
            active
            onClick={() => setCreateOpen(true)}
            disabled={activeCount >= MAX_ACTIVE_PROPERTIES}
          >
            {t(($) => $.properties.new_property)}
          </SettingsPillButton>
        ) : undefined
      }
    >
      <SettingsSection
        description={
          canManage
            ? t(($) => $.properties.limit_hint, {
                count: activeCount,
                max: MAX_ACTIVE_PROPERTIES,
              })
            : t(($) => $.properties.editor.admin_hint)
        }
        action={
          <div className="flex flex-wrap items-center justify-end gap-2">
            <SettingsPillButton
              active={showArchived}
              onClick={() => setShowArchived((current) => !current)}
            >
              {t(($) => $.properties.show_archived)}
            </SettingsPillButton>
            <SettingsSearchField
              value={query}
              onValueChange={setQuery}
              placeholder={t(($) => $.properties.search_placeholder)}
              className="w-full sm:w-52"
            />
          </div>
        }
      >
        <SettingsCard>
          {isLoading ? (
            <SettingsEmpty title={t(($) => $.properties.loading)} />
          ) : visible.length === 0 ? (
            <SettingsEmpty
              title={
                query
                  ? t(($) => $.properties.no_results)
                  : t(($) => $.properties.empty)
              }
              description={!query ? t(($) => $.properties.empty_hint) : undefined}
            />
          ) : (
            visible.map((property) => (
              <SettingsListRow key={property.id}>
                <div className="flex min-w-0 flex-1 items-center gap-2">
                  <PropertyIcon property={property} />
                  <div className="min-w-0">
                    <div className="flex min-w-0 items-center gap-2">
                      <span className="truncate text-body font-medium">{property.name}</span>
                      {property.archived && (
                        <Badge variant="outline" className="shrink-0 text-micro">
                          {t(($) => $.properties.archived_badge)}
                        </Badge>
                      )}
                    </div>
                    <p className="truncate text-caption text-muted-foreground">
                      <PropertyTypeLabel type={property.type} />
                    </p>
                  </div>
                </div>
                <div className="hidden min-w-0 flex-wrap items-center gap-1 md:flex">
                  {(property.config.options ?? []).slice(0, 6).map((option) => (
                    <span
                      key={option.id}
                      className="inline-flex items-center gap-1.5 rounded-full bg-muted px-2 py-0.5 text-caption"
                    >
                      <span
                        className="size-2 rounded-full"
                        style={{ backgroundColor: option.color }}
                      />
                      {option.name}
                    </span>
                  ))}
                  {(property.config.options?.length ?? 0) > 6 && (
                    <span className="text-caption text-muted-foreground">
                      +{(property.config.options?.length ?? 0) - 6}
                    </span>
                  )}
                </div>
                <span className="hidden shrink-0 text-caption text-muted-foreground md:inline">
                  {t(($) => $.properties.usage_count, { count: property.usage_count ?? 0 })}
                </span>
                {canManage ? (
                  <DropdownMenu>
                    <DropdownMenuTrigger
                      render={
                        <SettingsIconButton
                          aria-label={t(($) => $.properties.actions.open, { name: property.name })}
                        >
                          <MoreHorizontal className="size-4" />
                        </SettingsIconButton>
                      }
                    />
                    <DropdownMenuContent align="end">
                      <DropdownMenuItem onClick={() => setEditing(property)}>
                        <Pencil className="size-4" />
                        {t(($) => $.properties.actions.edit)}
                      </DropdownMenuItem>
                      {property.archived ? (
                        <DropdownMenuItem
                          onClick={() =>
                            update.mutate(
                              { id: property.id, archived: false },
                              {
                                onError: (error) =>
                                  toast.error(
                                    error instanceof Error
                                      ? error.message
                                      : t(($) => $.properties.save_failed),
                                  ),
                              },
                            )
                          }
                        >
                          <ArchiveRestore className="size-4" />
                          {t(($) => $.properties.actions.unarchive)}
                        </DropdownMenuItem>
                      ) : (
                        <DropdownMenuItem
                          variant="destructive"
                          onClick={() => setPendingArchive(property)}
                        >
                          <Archive className="size-4" />
                          {t(($) => $.properties.actions.archive)}
                        </DropdownMenuItem>
                      )}
                    </DropdownMenuContent>
                  </DropdownMenu>
                ) : null}
              </SettingsListRow>
            ))
          )}
        </SettingsCard>
      </SettingsSection>

      <PropertyEditorDialog open={createOpen} onOpenChange={setCreateOpen} />
      <PropertyEditorDialog
        open={Boolean(editing)}
        onOpenChange={(open) => !open && setEditing(null)}
        property={editing}
      />
      <ArchivePropertyDialog
        property={pendingArchive}
        onClose={() => setPendingArchive(null)}
      />
    </SettingsTab>
  );
}

export function PropertyTypeLabel({ type }: { type: string }) {
  const { t } = useT("settings");
  switch (type) {
    case "text":
      return <>{t(($) => $.properties.types.text)}</>;
    case "number":
      return <>{t(($) => $.properties.types.number)}</>;
    case "select":
      return <>{t(($) => $.properties.types.select)}</>;
    case "multi_select":
      return <>{t(($) => $.properties.types.multi_select)}</>;
    case "date":
      return <>{t(($) => $.properties.types.date)}</>;
    case "checkbox":
      return <>{t(($) => $.properties.types.checkbox)}</>;
    case "url":
      return <>{t(($) => $.properties.types.url)}</>;
    case "actor":
      return <>{t(($) => $.properties.types.actor)}</>;
    case "multi_actor":
      return <>{t(($) => $.properties.types.multi_actor)}</>;
    default:
      // Forward compat: newer servers may ship types this build doesn't know.
      return <>{type}</>;
  }
}

function PropertyEditorDialog({
  open,
  onOpenChange,
  property,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  property?: IssueProperty | null;
}) {
  const { t } = useT("settings");
  const create = useCreateProperty();
  const update = useUpdateProperty();
  const [draft, setDraft] = useState<PropertyDraft>(EMPTY_DRAFT);
  const [iconPickerOpen, setIconPickerOpen] = useState(false);

  useEffect(() => {
    if (!open) return;
    setIconPickerOpen(false);
    setDraft(
      property
        ? {
            name: property.name,
            type: (property.type as IssuePropertyType) ?? "text",
            description: property.description ?? "",
            icon: property.icon ?? "",
            options: (property.config.options ?? []).map((option: IssuePropertyOption) => ({
              id: option.id,
              name: option.name,
              color: option.color,
            })),
          }
        : EMPTY_DRAFT,
    );
  }, [property, open]);

  const showOptions = typeHasOptions(draft.type);
  const validOptions = draft.options.filter((option) => option.name.trim());
  const propertyTypeItems = ISSUE_PROPERTY_TYPES.map((type) => ({
    value: type,
    label: <PropertyTypeLabel type={type} />,
  }));
  const canSubmit =
    draft.name.trim().length > 0 && (!showOptions || validOptions.length > 0);

  const submit = () => {
    if (!canSubmit) return;
    const config = showOptions
      ? {
          options: validOptions.map((option) => ({
            id: option.id ?? "",
            name: option.name.trim(),
            color: option.color,
          })),
        }
      : undefined;
    const onError = (error: unknown) =>
      toast.error(
        error instanceof Error ? error.message : t(($) => $.properties.save_failed),
      );
    if (property) {
      update.mutate(
        {
          id: property.id,
          name: draft.name.trim(),
          description: draft.description.trim(),
          icon: draft.icon,
          ...(config ? { config } : {}),
        },
        { onSuccess: () => onOpenChange(false), onError },
      );
      return;
    }
    create.mutate(
      {
        name: draft.name.trim(),
        type: draft.type,
        description: draft.description.trim(),
        icon: draft.icon,
        ...(config ? { config } : {}),
      },
      { onSuccess: () => onOpenChange(false), onError },
    );
  };

  const setOption = (index: number, patch: Partial<OptionDraft>) => {
    setDraft((current) => ({
      ...current,
      options: current.options.map((option, i) =>
        i === index ? { ...option, ...patch } : option,
      ),
    }));
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>
            {property
              ? t(($) => $.properties.editor.edit_title)
              : t(($) => $.properties.editor.create_title)}
          </DialogTitle>
          <DialogDescription>
            {t(($) => $.properties.editor.admin_hint)}
          </DialogDescription>
        </DialogHeader>
        <div className="space-y-5 py-2">
          <div className="grid grid-cols-[4.25rem_minmax(0,1fr)_10rem] gap-3">
            <div className="space-y-2">
              <FieldLabel>{t(($) => $.properties.editor.icon)}</FieldLabel>
              <Popover open={iconPickerOpen} onOpenChange={setIconPickerOpen}>
                <PopoverTrigger
                  render={
                    <Button
                      type="button"
                      variant="outline"
                      className="w-full px-0 text-title"
                      aria-label={t(($) => $.properties.editor.choose_icon)}
                    >
                      {draft.icon ? (
                        <PropertyIconGlyph icon={draft.icon} />
                      ) : (
                        <SlidersHorizontal className="size-4 text-muted-foreground" />
                      )}
                    </Button>
                  }
                />
                <PopoverContent align="start" className="w-auto p-2">
                  <PropertyIconPicker
                    value={draft.icon}
                    label={t(($) => $.properties.editor.choose_icon)}
                    removeLabel={t(($) => $.properties.editor.remove_icon)}
                    onSelect={(icon) => {
                      setDraft((current) => ({ ...current, icon }));
                      setIconPickerOpen(false);
                    }}
                    onRemove={() => {
                      setDraft((current) => ({ ...current, icon: "" }));
                      setIconPickerOpen(false);
                    }}
                  />
                </PopoverContent>
              </Popover>
            </div>
            <div className="space-y-2">
              <FieldLabel htmlFor="property-name">
                {t(($) => $.properties.editor.name)}
              </FieldLabel>
              <Input
                id="property-name"
                autoFocus
                maxLength={32}
                value={draft.name}
                onChange={(event) =>
                  setDraft((current) => ({ ...current, name: event.target.value }))
                }
                placeholder={t(($) => $.properties.editor.name_placeholder)}
              />
            </div>
            <div className="space-y-2">
              <FieldLabel>{t(($) => $.properties.editor.type)}</FieldLabel>
              {property ? (
                <div
                  className="flex h-9 items-center rounded-md border border-surface-border px-3 text-body text-muted-foreground"
                  title={t(($) => $.properties.editor.type_locked_hint)}
                >
                  <PropertyTypeLabel type={property.type} />
                </div>
              ) : (
                <Select
                  items={propertyTypeItems}
                  value={draft.type}
                  onValueChange={(value) =>
                    value &&
                    setDraft((current) => ({
                      ...current,
                      type: value as IssuePropertyType,
                      options:
                        typeHasOptions(value) && current.options.length === 0
                          ? [{ name: "", color: COLOR_PICKER_PRESETS[6] }]
                          : current.options,
                    }))
                  }
                >
                  <SelectTrigger>
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {propertyTypeItems.map((item) => (
                      <SelectItem key={item.value} value={item.value}>
                        {item.label}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              )}
            </div>
          </div>
          <div className="space-y-2">
            <FieldLabel htmlFor="property-description">
              {t(($) => $.properties.editor.description)}
            </FieldLabel>
            <Textarea
              id="property-description"
              rows={2}
              value={draft.description}
              onChange={(event) =>
                setDraft((current) => ({ ...current, description: event.target.value }))
              }
              placeholder={t(($) => $.properties.editor.description_placeholder)}
            />
          </div>
          {showOptions && (
            <div className="space-y-2">
              <FieldLabel>{t(($) => $.properties.editor.options)}</FieldLabel>
              <div className="space-y-2">
                {draft.options.map((option, index) => (
                  <div key={option.id ?? index} className="flex items-center gap-2">
                    <GripVertical className="size-4 shrink-0 text-faint-foreground" />
                    <ColorPicker
                      value={option.color}
                      onChange={(color) => setOption(index, { color })}
                      trigger={
                        <button
                          type="button"
                          aria-label={option.color}
                          className="size-6 shrink-0 cursor-pointer rounded-full border border-surface-border transition-transform hover:scale-110"
                          style={{ backgroundColor: option.color }}
                        />
                      }
                    />
                    <Input
                      value={option.name}
                      maxLength={32}
                      onChange={(event) => setOption(index, { name: event.target.value })}
                      placeholder={t(($) => $.properties.editor.option_name_placeholder)}
                      className="h-8"
                    />
                    <Button
                      variant="ghost"
                      size="icon-sm"
                      aria-label={t(($) => $.properties.editor.remove_option)}
                      disabled={draft.options.length <= 1}
                      onClick={() =>
                        setDraft((current) => ({
                          ...current,
                          options: current.options.filter((_, i) => i !== index),
                        }))
                      }
                    >
                      <X className="size-4" />
                    </Button>
                  </div>
                ))}
              </div>
              <Button
                variant="ghost"
                size="sm"
                className="gap-2"
                onClick={() =>
                  setDraft((current) => ({
                    ...current,
                    options: [
                      ...current.options,
                      {
                        name: "",
                        color: COLOR_PICKER_PRESETS[current.options.length % COLOR_PICKER_PRESETS.length] ?? COLOR_PICKER_PRESETS[6],
                      },
                    ],
                  }))
                }
              >
                <Plus className="size-3.5" />
                {t(($) => $.properties.editor.add_option)}
              </Button>
            </div>
          )}
        </div>
        <DialogFooter>
          <Button variant="ghost" onClick={() => onOpenChange(false)}>
            {t(($) => $.properties.editor.cancel)}
          </Button>
          <Button
            onClick={submit}
            disabled={!canSubmit || create.isPending || update.isPending}
          >
            {create.isPending || update.isPending
              ? t(($) => $.properties.editor.saving)
              : t(($) => $.properties.editor.save)}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

function ArchivePropertyDialog({
  property,
  onClose,
}: {
  property: IssueProperty | null;
  onClose: () => void;
}) {
  const { t } = useT("settings");
  const update = useUpdateProperty();
  return (
    <AlertDialog open={Boolean(property)} onOpenChange={(open) => !open && onClose()}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>{t(($) => $.properties.archive_dialog.title)}</AlertDialogTitle>
          <AlertDialogDescription>
            {t(($) => $.properties.archive_dialog.description, {
              name: property?.name ?? "",
            })}
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>{t(($) => $.properties.archive_dialog.cancel)}</AlertDialogCancel>
          <AlertDialogAction
            onClick={() => {
              if (!property) return;
              update.mutate(
                { id: property.id, archived: true },
                {
                  onSuccess: onClose,
                  onError: (error) =>
                    toast.error(
                      error instanceof Error
                        ? error.message
                        : t(($) => $.properties.archive_dialog.failed),
                    ),
                },
              );
            }}
          >
            {t(($) => $.properties.archive_dialog.confirm)}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
