"use client";

import { useEffect } from "react";
import { isRuntimeUsableForUser } from "@patchbay/core/runtimes";
import type { AgentRuntime } from "@patchbay/core/types";
import { ModelDropdown } from "../../agents/components/model-dropdown";
import { CompactRuntimeRow } from "./compact-runtime-row";

export interface PatrickRuntimeSelection {
  runtimeId: string;
  /** Empty means "whatever the runtime defaults to". */
  model: string;
}

/**
 * The one place that asks "which runtime should Patrick use, and on which model".
 *
 * Three surfaces reach this same decision — desktop onboarding, the web CLI
 * dialog, and the Runtimes page — and they had drifted: two offered a model
 * and one did not, so connecting via the web CLI silently created Patrick on the
 * runtime's default model while desktop let you choose. Two of them also
 * re-implemented the reset rule below and the third simply lacked it.
 *
 * `layout` exists because the presentation genuinely differs, not because the
 * logic does. The CLI dialog lists machines because that is the moment they
 * appear one at a time after `patchbay setup`, and a collapsed dropdown hides
 * exactly the feedback that dialog is there to give.
 */
export function PatrickRuntimeChoice({
  runtimes,
  runtimesLoading,
  currentUserId = null,
  value,
  onChange,
  disabled = false,
  layout = "dropdown",
}: {
  runtimes: AgentRuntime[];
  runtimesLoading?: boolean;
  /** Only used by the dropdown layout, to label runtime owners. */
  currentUserId?: string | null;
  value: PatrickRuntimeSelection;
  onChange: (next: PatrickRuntimeSelection) => void;
  disabled?: boolean;
  layout?: "dropdown" | "list";
}) {
  const usableRuntimes = runtimes.filter((runtime) =>
    isRuntimeUsableForUser(runtime, currentUserId),
  );
  useEffect(() => {
    if (value.runtimeId || disabled || runtimesLoading) return;
    const candidate = runtimes.find(
      (runtime) =>
        runtime.status === "online" &&
        isRuntimeUsableForUser(runtime, currentUserId),
    );
    if (candidate) onChange({ runtimeId: candidate.id, model: "" });
  }, [
    value.runtimeId,
    disabled,
    runtimesLoading,
    runtimes,
    currentUserId,
    onChange,
  ]);
  const selected = runtimes.find((rt) => rt.id === value.runtimeId) ?? null;

  const selectRuntime = (runtimeId: string) => {
    // Models are per-runtime, so a value chosen for the previous runtime may
    // not exist on this one. Owned here so no caller can forget it.
    onChange({
      runtimeId,
      model: runtimeId === value.runtimeId ? value.model : "",
    });
  };

  return (
    <div className="flex flex-col gap-3">
      {layout === "list" ? (
        // Capped at ~4 rows so the install commands above stay reachable when
        // a member has many machines registered.
        <div className="flex max-h-[240px] flex-col gap-2 overflow-y-auto">
          {usableRuntimes.map((rt) => (
            <CompactRuntimeRow
              key={rt.id}
              runtime={rt}
              selected={rt.id === value.runtimeId}
              onSelect={() => selectRuntime(rt.id)}
              disabled={disabled}
            />
          ))}
        </div>
      ) : null}
      <ModelDropdown
        variant="chip"
        allowEffort={false}
        provider={selected?.provider}
        runtimes={usableRuntimes}
        onSelection={({ runtimeId, model }) => onChange({ runtimeId, model })}
        runtimeId={value.runtimeId || null}
        runtimeOnline={selected?.status === "online"}
        value={value.model}
        onChange={(model) => onChange({ ...value, model })}
        disabled={runtimesLoading || disabled}
      />
    </div>
  );
}
