import type { RuntimeSessionMode } from "../types/agent";

/**
 * Session-mode rows the Agent composer picker may show.
 *
 * Full access is NOT in this list: the UI always synthesizes it as the empty
 * persisted value (daemon yolo / bypass). Ask, read-only, plan, and similar
 * restricted choices stay out of the picker even when a protocol advertises
 * them. Matching is by advertised `kind` / `value`, never by provider name.
 */
export function pickerSessionModes(
  advertised: readonly RuntimeSessionMode[] | undefined,
): RuntimeSessionMode[] {
  if (!advertised || advertised.length === 0) return [];
  const seen = new Set<string>();
  const modes: RuntimeSessionMode[] = [];
  for (const mode of advertised) {
    if (!isPickerSessionMode(mode)) continue;
    const key = mode.value.trim();
    if (!key || seen.has(key)) continue;
    seen.add(key);
    modes.push({
      value: mode.value,
      label: mode.label.trim() || mode.value,
      kind: mode.kind,
    });
  }
  return modes;
}

export function isPickerSessionMode(mode: RuntimeSessionMode): boolean {
  const value = mode.value.trim().toLowerCase();
  const kind = (mode.kind ?? "").trim().toLowerCase();
  if (!value || isExcludedSessionMode(value, kind)) return false;
  return kind === "auto_review" || value === "auto";
}

function isExcludedSessionMode(value: string, kind: string): boolean {
  if (value === "auto" || kind === "auto_review") return false;
  return (
    kind === "ask" ||
    kind === "read_only" ||
    kind === "readonly" ||
    kind === "plan" ||
    value === "ask" ||
    value === "read-only" ||
    value === "read_only" ||
    value === "plan" ||
    value === "default" ||
    value === "acceptedits" ||
    value === "dontask" ||
    value === "bypasspermissions" ||
    value === "yolo"
  );
}
