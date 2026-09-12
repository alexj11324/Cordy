"use client";

import { memo, useCallback, useMemo, useState, type KeyboardEvent } from "react";
import { Keyboard, Search } from "lucide-react";
import { Button, Input } from "@lobehub/ui/base-ui";
import { cn } from "@orvilo/ui/lib/utils";
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
} from "@orvilo/core/shortcuts";
import { isImeComposing } from "@orvilo/core/utils";
import { useT } from "../../i18n";
import { ShortcutKeycaps } from "../../common/shortcut-keycaps";
import { SettingsEmptyState } from "./settings-empty";
import { SettingsFormRow, SettingsGroup } from "./settings-shell";
import { useSettingsConfirm } from "./settings-confirm";

type CaptureError =
  | { kind: "conflict"; actionId: ShortcutActionId }
  | { kind: "reserved" }
  | { kind: "send" }
  | { kind: "unsafe" }
  | null;

/** Width of the control column: the recorder plus Reset and Disable. */
const SHORTCUT_CONTROL_MIN_WIDTH = 288;

/**
 * Keyboard shortcuts — a search box, one group per category, and the fixed
 * bindings the app does not let you rebind.
 *
 * "Restore defaults" is the tab's page-level action and it now lives on the
 * first group's `extra`. It used to be passed to `SettingsTab` as `action`,
 * which the settings dialog never rendered: `SettingsTab` returns early when
 * it is nested in `SettingsDialogBody` (`settings-layout.tsx`), and that branch
 * renders `children` and nothing else, so the button was dropped for every user
 * inside the dialog while the standalone tests kept passing. `Form.Group`'s
 * `extra` renders wherever the group sits, which is what restores it here.
 *
 * The search box is Lobe's `Input` rather than its `SearchBar`, for the reason
 * the settings rail already gives: `SearchBar` renders its clear affordance as
 * an unnamed icon button, and this field wants escape-to-clear as well. Its
 * value lives in this component's state (`query`), not in any form store.
 */
export function KeyboardShortcutsTab() {
  const { t } = useT("settings");
  const [query, setQuery] = useState("");
  const [recording, setRecording] = useState<ShortcutActionId | null>(null);
  const [captureError, setCaptureError] = useState<CaptureError>(null);
  const overrides = useShortcutStore((state) => state.overrides);
  const setShortcut = useShortcutStore((state) => state.setShortcut);
  const resetShortcut = useShortcutStore((state) => state.resetShortcut);
  const resetAll = useShortcutStore((state) => state.resetAll);
  const confirm = useSettingsConfirm();

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

  const searchPlaceholder = t(($) => $.shortcuts.search_placeholder);
  const groups: readonly ShortcutCategory[] = ["general", "navigation"];

  const capture = useCallback((actionId: ShortcutActionId, event: KeyboardEvent) => {
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
  }, [setShortcut]);

  // One stable handler bag for every row. `capture` and the store actions are
  // stable, so this object is too — which is what lets `ShortcutRow`'s memo
  // hold. Without it a single keystroke re-renders all 23 rows, and each row
  // carries three `motion`-backed controls; with it only the row being edited
  // does any work. That is worth more in the app than in the suite, but the
  // suite is where it is measurable: one tab mount there costs ~6s.
  const handlers = useMemo(
    () => ({
      onStartRecording: (actionId: ShortcutActionId) => {
        setRecording(actionId);
        setCaptureError(null);
      },
      onCancelRecording: () => {
        setRecording(null);
        setCaptureError(null);
      },
      onCapture: capture,
      onDisable: (actionId: ShortcutActionId) => {
        setShortcut(actionId, null);
        setCaptureError(null);
      },
      onReset: (actionId: ShortcutActionId) => {
        resetShortcut(actionId);
        setCaptureError(null);
      },
    }),
    [capture, resetShortcut, setShortcut],
  );

  const openResetConfirm = () => {
    confirm({
      title: t(($) => $.shortcuts.reset_confirm.title),
      description: t(($) => $.shortcuts.reset_confirm.description),
      confirmLabel: t(($) => $.shortcuts.reset_confirm.confirm),
      cancelLabel: t(($) => $.shortcuts.reset_confirm.cancel),
      // Synchronous store write, but `useSettingsConfirm` requires the promise
      // so the dialog cannot close before the action it confirms.
      onConfirm: async () => {
        resetAll();
        setCaptureError(null);
        setRecording(null);
      },
    });
  };

  return (
    <>
      <SettingsGroup
        variant="borderless"
        extra={
          <Button
            shape="round"
            disabled={Object.keys(overrides).length === 0}
            onClick={openResetConfirm}
          >
            {t(($) => $.shortcuts.reset_all)}
          </Button>
        }
      >
        <Input
          role="searchbox"
          aria-label={searchPlaceholder}
          className="max-w-sm"
          placeholder={searchPlaceholder}
          prefix={<Search aria-hidden="true" className="size-3.5" />}
          value={query}
          onChange={(event) => setQuery(event.target.value)}
          onKeyDown={(event) => {
            // The dialog closes on Escape; a search the user can see should
            // clear first, so this stops the event before it reaches it.
            if (event.key === "Escape" && query) {
              event.stopPropagation();
              setQuery("");
            }
          }}
        />
      </SettingsGroup>

      {groups.map((category) => {
        const actions = visibleActions.filter(
          (action) => action.category === category,
        );
        if (actions.length === 0) return null;
        return (
          <SettingsGroup
            key={category}
            title={t(($) => $.shortcuts.categories[category])}
          >
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
                handlers={handlers}
              />
            ))}
          </SettingsGroup>
        );
      })}

      {visibleActions.length === 0 ? (
        <SettingsEmptyState title={t(($) => $.shortcuts.no_results)} />
      ) : null}

      <SettingsGroup
        title={t(($) => $.shortcuts.fixed.title)}
        description={t(($) => $.shortcuts.fixed.description)}
      >
        <FixedShortcutRow label={t(($) => $.shortcuts.fixed.open_settings)} shortcut={createShortcutChord(",", { primary: true })} />
        <FixedShortcutRow label={t(($) => $.shortcuts.fixed.close_tab)} shortcut={createShortcutChord("W", { primary: true })} />
        <FixedShortcutRow label={t(($) => $.shortcuts.fixed.zoom_in)} shortcut={createShortcutChord("Plus", { primary: true })} />
        <FixedShortcutRow label={t(($) => $.shortcuts.fixed.zoom_out)} shortcut={createShortcutChord("Minus", { primary: true })} />
        <FixedShortcutRow label={t(($) => $.shortcuts.fixed.reset_zoom)} shortcut={createShortcutChord("0", { primary: true })} />
        <FixedShortcutRow label={t(($) => $.shortcuts.fixed.close_dialog)} shortcut={createShortcutChord("Escape")} />
      </SettingsGroup>
    </>
  );
}

interface ShortcutRowHandlers {
  onStartRecording: (actionId: ShortcutActionId) => void;
  onCancelRecording: () => void;
  onCapture: (actionId: ShortcutActionId, event: KeyboardEvent) => void;
  onDisable: (actionId: ShortcutActionId) => void;
  onReset: (actionId: ShortcutActionId) => void;
}

/**
 * Memoized on purpose: the props are either stable (`handlers`, `action`) or
 * change only for the row being edited (`recording`, `error`), so recording a
 * shortcut no longer re-renders every other row's motion-backed controls.
 */
const ShortcutRow = memo(function ShortcutRow({
  action,
  shortcut,
  customized,
  recording,
  error,
  handlers,
}: {
  action: ShortcutActionDefinition;
  shortcut: ShortcutChord | null;
  customized: boolean;
  recording: boolean;
  error: CaptureError;
  handlers: ShortcutRowHandlers;
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
    <SettingsFormRow
      label={label}
      description={
        action.id === "send" && isPlainShortcut(shortcut, "Enter") ? (
          <>
            {description}
            <span className="text-brand mt-1 block">
              {t(($) => $.shortcuts.send_enter_hint)}
            </span>
          </>
        ) : description
      }
      minWidth={SHORTCUT_CONTROL_MIN_WIDTH}
      align="start"
    >
      <div className="flex flex-col items-stretch gap-1.5 sm:items-end">
        <div className="flex flex-wrap items-center justify-end gap-1.5">
          <button
            type="button"
            onClick={() => handlers.onStartRecording(action.id)}
            onKeyDown={
              recording
                ? (event) => handlers.onCapture(action.id, event)
                : undefined
            }
            onBlur={handlers.onCancelRecording}
            className={cn(
              "min-w-28 text-body inline-flex h-8 items-center justify-center rounded-full bg-muted px-3 font-medium outline-none transition-colors hover:bg-muted/80 focus-visible:ring-2 focus-visible:ring-ring",
              recording && "bg-primary/10 text-primary ring-primary/20 ring-2",
              error && "bg-destructive/10 text-destructive ring-destructive/20 ring-2",
            )}
            aria-label={t(($) => $.shortcuts.record_aria, { action: label })}
            aria-pressed={recording}
            data-shortcut-recording={recording ? "" : undefined}
          >
            {recording ? (
              <span className="inline-flex items-center gap-1.5">
                <Keyboard className="size-3.5" />
                {t(($) => $.shortcuts.recording)}
              </span>
            ) : shortcut ? (
              <ShortcutKeycaps shortcut={shortcut} decorative />
            ) : (
              <span className="text-muted-foreground font-normal">
                {t(($) => $.shortcuts.unassigned)}
              </span>
            )}
          </button>
          <Button
            shape="round"
            type="fill"
            onClick={() => handlers.onReset(action.id)}
            disabled={!customized}
            aria-label={t(($) => $.shortcuts.reset_action, { action: label })}
          >
            {t(($) => $.shortcuts.reset)}
          </Button>
          <Button
            shape="round"
            type="fill"
            onClick={() => handlers.onDisable(action.id)}
            disabled={shortcut === null}
            aria-label={t(($) => $.shortcuts.disable_action, { action: label })}
          >
            {t(($) => $.shortcuts.disable)}
          </Button>
        </div>
        {errorText ? (
          <span role="alert" className="max-w-72 text-caption text-destructive text-right">
            {errorText}
          </span>
        ) : recording ? (
          <span className="text-micro text-muted-foreground text-right">
            {t(($) => $.shortcuts.record_hint)}
          </span>
        ) : null}
      </div>
    </SettingsFormRow>
  );
});

function FixedShortcutRow({ label, shortcut }: { label: string; shortcut: ShortcutChord }) {
  return (
    <SettingsFormRow label={label}>
      <ShortcutKeycaps shortcut={shortcut} size="md" className="justify-end" />
    </SettingsFormRow>
  );
}
