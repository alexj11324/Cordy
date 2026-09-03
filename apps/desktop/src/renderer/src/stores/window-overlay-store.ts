import { create } from "zustand";
import { standaloneSettingsWorkspaceSlug } from "@/platform/standalone-settings";

/**
 * Window-level destinations that are NOT pages inside a tab. This includes
 * pre-workspace transitions and the first-class Settings page. Triggered by
 * navigation-adapter interception, zero-workspace auto-redirect, shortcuts,
 * or deep links; rendered above the tab system as a full-window takeover.
 *
 * These flows used to be routes (`/workspaces/new`, `/invite/:id`) but on
 * desktop the URL is invisible to users — routes are an implementation detail
 * of the tab system. Representing transitions as routes meant tabs tried to
 * persist them, TabBar rendered on top, and invite deep-linking had no clean
 * dispatch target. Modeling them as application state removes all three.
 */
export type WindowOverlay =
  | { type: "new-workspace" }
  | { type: "invite"; invitationId: string }
  | { type: "invitations" }
  | { type: "onboarding" }
  | { type: "settings"; path: string };

interface WindowOverlayStore {
  overlay: WindowOverlay | null;
  open: (overlay: WindowOverlay) => void;
  close: () => void;
  validateSettingsWorkspace: (validSlugs: Set<string>) => void;
}

export const useWindowOverlayStore = create<WindowOverlayStore>((set) => ({
  overlay: null,
  open: (overlay) => set({ overlay }),
  close: () => set({ overlay: null }),
  validateSettingsWorkspace: (validSlugs) =>
    set((state) => {
      if (state.overlay?.type !== "settings") return state;
      const slug = standaloneSettingsWorkspaceSlug(state.overlay.path);
      return slug && validSlugs.has(slug) ? state : { overlay: null };
    }),
}));
