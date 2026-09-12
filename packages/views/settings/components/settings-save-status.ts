/**
 * Lifecycle of a settings field that persists on its own.
 *
 * Kept apart from `settings-layout` so `use-auto-save` and the tabs that read
 * the status do not have to import the hand-rolled settings kit to name it.
 * The kit is on its way out — every tab is migrating to Lobe UI — while this
 * contract survives the migration, and a type that outlives its old home
 * should not move with it.
 */
export type SettingsSaveStatus = "idle" | "saving" | "saved" | "error";
