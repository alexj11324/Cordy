import { create } from "zustand";

export interface AgentThreadPanelSelection {
  workspaceId: string;
  taskId: string;
  routePath: string;
}

interface AgentThreadPanelState {
  panel: AgentThreadPanelSelection | null;
  openPanel: (selection: AgentThreadPanelSelection) => void;
  closePanel: () => void;
}

export const useAgentThreadPanelStore = create<AgentThreadPanelState>((set) => ({
  panel: null,
  openPanel: (panel) => set({ panel }),
  closePanel: () => set({ panel: null }),
}));
