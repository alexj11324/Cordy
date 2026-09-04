"use client";

import { useMemo, useState, type KeyboardEvent } from "react";
import { Keyboard } from "lucide-react";
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
import { cn } from "@patchbay/ui/lib/utils";
import {
  findShortcutConflict,
  createShortcutChord,
  isReservedShortcut,
  isShortcutAllowedForAction,
  isPlainShortcut,
  resolveShortcut,
  shortcutFromEvent,
  SHORTCUT_ACTIONS,
  useShortcutStore,
  type ShortcutActionDefinition,
  type ShortcutActionId,
  type ShortcutCategory,
  type ShortcutChord,
} from "@patchbay/core/shortcuts";
import { isImeComposing } from "@patchbay/core/utils";
import { useT } from "../../i18n";
import { ShortcutKeycaps } from "../../common/shortcut-keycaps";
import {
  SettingsCard,
  SettingsEmpty,
  SettingsPillButton,
  SettingsRow,
  SettingsSearchField,
  SettingsSection,
  SettingsTab,
} from "./settings-layout";

type CaptureError =
  | { kind: "conflict"; actionId: ShortcutActionId }
  | { kind: "reserved" }
  | { kind: "send" }
  | { kind: "unsafe" }
  | null;

export function KeyboardShortcutsTab() {
  const { t } = useT("settings");
  const [query, setQuery] = useState("");
  const [recording, setRecording] = useState<ShortcutActionId | null>(null);
  const [captureError, setCaptureError] = useState<CaptureError>(null);
  const [resetConfirmOpen, setResetConfirmOpen] = useState(false);
  const overrides = useShortcutStore((state) => state.overrides);
  const setShortcut = useShortcutStore((state) => state.setShortcut);
  const resetShortcut = useShortcutStore((state) => state.resetShortcut);
  const resetAll = useShortcutStore((state) => state.resetAll);

  const visibleActions = useMemo(() => {
    const needle = query.trim().toLocaleLowerCase();
    if (!needle) return SHORTCUT_ACTIONS;
    return SHORTCUT_ACTIONS.filter((action) => {
      const label = t(($) => $.shortcuts.actions[action.id].label);
      const description = t(
        ($) => $.shortcuts.actions[action.id].description,
      );
      return `${label} ${description}`
        .toLocaleLowerCase()
        .includes(needle);
    });
  }, [query, t]);

  const groups: readonly ShortcutCategory[] = ["general", "navigation"];

  const capture = (actionId: ShortcutActionId, event: KeyboardEvent) => {
    event.preventDefault();
    event.stopPropagation();
    if (event.repeat || isImeComposing(event)) return;
    if (event.key === "Escape") {
      setRecording(null);
      setCaptureError(null);
      return;
    }
    if (event.key === "Backspace" || event.key === "Delete") {
      setShortcut(actionId, null);
      setRecording(null);
      setCaptureError(null);
      return;
    }

    const shortcut = shortcutFromEvent(event.nativeEvent);
    if (!shortcut) return;
    if (isReservedShortcut(shortcut)) {
      setCaptureError({ kind: "reserved" });
      return;
    }
    if (!isShortcutAllowedForAction(actionId, shortcut)) {
      setCaptureError({ kind: actionId === "send" ? "send" : "unsafe" });
      return;
    }
    const conflict = findShortcutConflict(actionId, shortcut);
    if (conflict) {
      setCaptureError({ kind: "conflict", actionId: conflict });
      return;
    }

    setShortcut(actionId, shortcut);
    setRecording(null);
    setCaptureError(null);
  };

  return (
    <SettingsTab
      title={t(($) => $.shortcuts.title)}
      description={t(($) => $.shortcuts.description)}
      action={
        <SettingsPillButton
          onClick={() => setResetConfirmOpen(true)}
          disabled={Object.keys(overrides).length === 0}
        >
          {t(($) => $.shortcuts.reset_all)}
        </SettingsPillButton>
      }
    >
      <SettingsSearchField
        value={query}
        onValueChange={setQuery}
        placeholder={t(($) => $.shortcuts.search_placeholder)}
        className="max-w-sm"
      />

      {groups.map((category) => {
        const actions = visibleActions.filter(
          (action) => action.category === category,
        );
        if (actions.length === 0) return null;
        return (
          <SettingsSection
            key={category}
            title={t(($) => $.shortcuts.categories[category])}
          >
            <SettingsCard>
              {actions.map((action) => (
                <ShortcutRow
                  key={action.id}
                  action={action}
                  shortcut={resolveShortcut(overrides, action.id)}
                  customized={Object.prototype.hasOwnProperty.call(
                    overrides,
                    action.id,
                  )}
                  recording={recording === action.id}
                  error={recording === action.id ? captureError : null}
                  onStartRecording={() => {
                    setRecording(action.id);
                    setCaptureError(null);
                  }}
                  onCancelRecording={() => {
                    setRecording(null);
                    setCaptureError(null);
                  }}
                  onCapture={(event) => capture(action.id, event)}
                  onDisable={() => {
                    setShortcut(action.id, null);
                    setCaptureError(null);
                  }}
                  onReset={() => {
                    resetShortcut(action.id);
                    setCaptureError(null);
                  }}
                />
              ))}
            </SettingsCard>
          </SettingsSection>
        );
      })}

      {visibleActions.length === 0 ? (
        <SettingsEmpty title={t(($) => $.shortcuts.no_results)} />
      ) : null}

      <SettingsSection
        title={t(($) => $.shortcuts.fixed.title)}
        description={t(($) => $.shortcuts.fixed.description)}
      >
        <SettingsCard>
          <FixedShortcutRow label={t(($) => $.shortcuts.fixed.open_settings)} shortcut={createShortcutChord(",", { primary: true })} />
          <FixedShortcutRow label={t(($) => $.shortcuts.fixed.close_tab)} shortcut={createShortcutChord("W", { primary: true })} />
          <FixedShortcutRow label={t(($) => $.shortcuts.fixed.zoom_in)} shortcut={createShortcutChord("Plus", { primary: true })} />
          <FixedShortcutRow label={t(($) => $.shortcuts.fixed.zoom_out)} shortcut={createShortcutChord("Minus", { primary: true })} />
          <FixedShortcutRow label={t(($) => $.shortcuts.fixed.reset_zoom)} shortcut={createShortcutChord("0", { primary: true })} />
          <FixedShortcutRow label={t(($) => $.shortcuts.fixed.close_dialog)} shortcut={createShortcutChord("Escape")} />
        </SettingsCard>
      </SettingsSection>

      <AlertDialog open={resetConfirmOpen} onOpenChange={setResetConfirmOpen}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>
              {t(($) => $.shortcuts.reset_confirm.title)}
            </AlertDialogTitle>
            <AlertDialogDescription>
              {t(($) => $.shortcuts.reset_confirm.description)}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>
              {t(($) => $.shortcuts.reset_confirm.cancel)}
            </AlertDialogCancel>
            <AlertDialogAction
              variant="destructive"
              onClick={() => {
                resetAll();
                setCaptureError(null);
                setRecording(null);
                setResetConfirmOpen(false);
              }}
            >
              {t(($) => $.shortcuts.reset_confirm.confirm)}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </SettingsTab>
  );
}

function ShortcutRow({
  action,
  shortcut,
  customized,
  recording,
  error,
  onStartRecording,
  onCancelRecording,
  onCapture,
  onDisable,
  onReset,
}: {
  action: ShortcutActionDefinition;
  shortcut: ShortcutChord | null;
  customized: boolean;
  recording: boolean;
  error: CaptureError;
  onStartRecording: () => void;
  onCancelRecording: () => void;
  onCapture: (event: KeyboardEvent) => void;
  onDisable: () => void;
  onReset: () => void;
}) {
  const { t } = useT("settings");
  const label = t(($) => $.shortcuts.actions[action.id].label);
  const description = t(($) => $.shortcuts.actions[action.id].description);
  const errorText = error?.kind === "reserved"
    ? t(($) => $.shortcuts.reserved_error)
    : error?.kind === "send"
      ? t(($) => $.shortcuts.send_error)
    : error?.kind === "unsafe"
      ? t(($) => $.shortcuts.unsafe_error)
    : error?.kind === "conflict"
      ? t(($) => $.shortcuts.conflict_error, {
          action: t(($) => $.shortcuts.actions[error.actionId].label),
        })
      : null;

  return (
    <SettingsRow
      label={label}
      description={
        action.id === "send" && isPlainShortcut(shortcut, "Enter") ? (
          <>
            {description}
            <span className="mt-1 block text-brand">
              {t(($) => $.shortcuts.send_enter_hint)}
            </span>
          </>
        ) : description
      }
      align="start"
      size="select-wide"
    >
      <div className="flex flex-col items-stretch gap-1.5 sm:items-end">
        <div className="flex flex-wrap items-center justify-end gap-1.5">
          <button
            type="button"
            onClick={onStartRecording}
            onKeyDown={recording ? onCapture : undefined}
            onBlur={onCancelRecording}
            className={cn(
              "inline-flex h-8 min-w-28 items-center justify-center rounded-full bg-muted px-3 text-body font-medium outline-none transition-colors hover:bg-muted/80 focus-visible:ring-2 focus-visible:ring-ring",
              recording && "bg-primary/10 text-primary ring-2 ring-primary/20",
              error && "bg-destructive/10 text-destructive ring-2 ring-destructive/20",
            )}
            aria-label={t(($) => $.shortcuts.record_aria, { action: label })}
            aria-pressed={recording}
          >
            {recording ? (
              <span className="inline-flex items-center gap-1.5">
                <Keyboard className="size-3.5" />
                {t(($) => $.shortcuts.recording)}
              </span>
            ) : shortcut ? (
              <ShortcutKeycaps shortcut={shortcut} decorative />
            ) : (
              <span className="font-normal text-muted-foreground">
                {t(($) => $.shortcuts.unassigned)}
              </span>
            )}
          </button>
          <SettingsPillButton
            onClick={onReset}
            disabled={!customized}
            aria-label={t(($) => $.shortcuts.reset_action, { action: label })}
          >
            {t(($) => $.shortcuts.reset)}
          </SettingsPillButton>
          <SettingsPillButton
            onClick={onDisable}
            disabled={shortcut === null}
            aria-label={t(($) => $.shortcuts.disable_action, { action: label })}
          >
            {t(($) => $.shortcuts.disable)}
          </SettingsPillButton>
        </div>
        {errorText ? (
          <span role="alert" className="max-w-72 text-right text-caption text-destructive">
            {errorText}
          </span>
        ) : recording ? (
          <span className="text-right text-micro text-muted-foreground">
            {t(($) => $.shortcuts.record_hint)}
          </span>
        ) : null}
      </div>
    </SettingsRow>
  );
}

function FixedShortcutRow({ label, shortcut }: { label: string; shortcut: ShortcutChord }) {
  return (
    <SettingsRow label={label}>
      <ShortcutKeycaps shortcut={shortcut} size="md" className="justify-end" />
    </SettingsRow>
  );
}
