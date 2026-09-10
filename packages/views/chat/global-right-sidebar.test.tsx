import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { create } from "zustand";

const state = vi.hoisted(() => ({ pathname: "/acme/issues" }));
const useStore = create<{ isOpen: boolean; toggle: () => void }>((set) => ({
  isOpen: false,
  toggle: () => set((current) => ({ isOpen: !current.isOpen })),
}));
const agentStore = create<{
  panel: { workspaceId: string; taskId: string; routePath: string } | null;
  closePanel: () => void;
}>((set) => ({
  panel: null,
  closePanel: () => set({ panel: null }),
}));
vi.mock("@orvilo/core/chat", () => ({ useChatStore: (selector: Parameters<typeof useStore>[0]) => useStore(selector) }));
vi.mock("@orvilo/core/agent-thread", () => ({ useAgentThreadPanelStore: (selector: Parameters<typeof agentStore>[0]) => agentStore(selector) }));
vi.mock("@orvilo/core/paths", () => ({
  useWorkspacePaths: () => ({ chat: () => "/acme/chat" }),
  useCurrentWorkspace: () => ({ id: "workspace-1" }),
}));
vi.mock("../navigation", () => ({ useNavigation: () => state }));
vi.mock("../i18n", () => ({ useT: () => ({ t: (selector: (keys: unknown) => string) => selector({ sidebar: { title: "Agent chat", open: "Open right sidebar", close: "Close right sidebar", chat_page_notice: "Conversation is open in Chat" } }) }) }));
vi.mock("./components/chat-window", () => ({ ChatWindow: ({ docked }: { docked: boolean }) => <div data-testid="chat-content" data-docked={String(docked)} /> }));
vi.mock("../agent-thread/components/task-agent-thread-panel", () => ({
  TaskAgentThreadPanel: ({ taskId }: { taskId: string }) => <div data-testid="agent-thread-content">{taskId}</div>,
}));

const { GlobalRightSidebar, GlobalRightSidebarToggle } = await import("./global-right-sidebar");
function Shell() { return <><GlobalRightSidebarToggle /><GlobalRightSidebar /></>; }

beforeEach(() => { useStore.setState({ isOpen: false }); agentStore.setState({ panel: null }); state.pathname = "/acme/issues"; });
describe("global right sidebar", () => {
  it("keeps the header control mounted while opening and closing an in-flow chat column", () => {
    render(<Shell />);
    const composer = screen.getByTestId("chat-content");
    expect(document.getElementById("global-right-sidebar")).toHaveAttribute("hidden");
    fireEvent.click(screen.getByRole("button", { name: "Open right sidebar" }));
    expect(screen.getByRole("complementary", { name: "Agent chat" })).toHaveClass("shrink-0", "border-l");
    expect(screen.getByTestId("chat-content")).toHaveAttribute("data-docked", "true");
    const close = screen.getByRole("button", { name: "Close right sidebar" });
    expect(close).toHaveAttribute("aria-expanded", "true");
    expect(close.closest("header")?.parentElement).toBe(screen.getByRole("complementary"));
    expect(screen.queryByRole("button", { name: "Open right sidebar" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Close right sidebar" }));
    expect(screen.getByRole("button", { name: "Open right sidebar" })).toHaveAttribute("aria-expanded", "false");
    expect(screen.getByTestId("chat-content")).toBe(composer);
  });
  it("preserves open state across routes and avoids a duplicate composer on Chat", () => {
    const view = render(<Shell />);
    fireEvent.click(screen.getByRole("button", { name: "Open right sidebar" }));
    state.pathname = "/acme/agents/agent-1"; view.rerender(<Shell />);
    expect(screen.getByTestId("chat-content")).toBeInTheDocument();
    state.pathname = "/acme/chat/session-1"; view.rerender(<Shell />);
    expect(screen.queryByTestId("chat-content")).toBeNull();
    expect(screen.getByText("Conversation is open in Chat")).toBeInTheDocument();
    state.pathname = "/acme/projects"; view.rerender(<Shell />);
    expect(screen.getByTestId("chat-content")).toBeInTheDocument();
    expect(useStore.getState().isOpen).toBe(true);
  });
  it("renders the selected Agent thread in the same global column", () => {
    const view = render(<Shell />);
    agentStore.setState({
      panel: { workspaceId: "workspace-1", taskId: "task-1", routePath: "/acme/issues" },
    });
    view.rerender(<Shell />);
    expect(screen.getByTestId("agent-thread-content")).toHaveTextContent("task-1");
    expect(screen.queryByTestId("chat-content")).toBeNull();
    expect(screen.getByRole("complementary", { name: "Agent chat" })).not.toHaveAttribute("hidden");
  });
});
