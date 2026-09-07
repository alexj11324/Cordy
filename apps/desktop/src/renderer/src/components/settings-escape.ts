import { isImeComposing } from "@patchbay/core/utils";

/** Open Base UI layers that own Escape before Settings does. */
const OPEN_LAYER_SELECTOR = [
  '[data-slot="dialog-content"][data-open]',
  '[data-slot="alert-dialog-content"][data-open]',
  '[data-slot="popover-content"][data-open]',
  '[data-slot="dropdown-menu-content"][data-open]',
  '[data-slot="select-content"][data-open]',
  '[data-slot="combobox-content"][data-open]',
  '[role="menu"][data-open]',
  '[role="listbox"][data-open]',
].join(",");

/**
 * Settings overlay Escape ownership. Inner dialogs, menus, shortcut
 * recording, and IME composition must cancel themselves first; this
 * returns true only when Escape should close Settings itself.
 */
export function shouldCloseSettingsOnEscape(event: KeyboardEvent): boolean {
  if (event.key !== "Escape") return false;
  if (event.defaultPrevented) return false;
  if (event.altKey || event.ctrlKey || event.metaKey) return false;
  if (isImeComposing(event)) return false;
  if (typeof Element !== "undefined" && event.target instanceof Element) {
    if (event.target.closest("[data-shortcut-recording]")) return false;
  }
  if (
    typeof document !== "undefined" &&
    document.querySelector(OPEN_LAYER_SELECTOR)
  ) {
    return false;
  }
  return true;
}
