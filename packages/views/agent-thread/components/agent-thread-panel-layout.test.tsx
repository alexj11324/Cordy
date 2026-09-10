// @vitest-environment jsdom
import { act, fireEvent, render, screen } from "@testing-library/react";
import { expect, it } from "vitest";
import { useAgentThreadPanelStore } from "@orvilo/core/agent-thread";
import { AgentThreadPanelLayout } from "./agent-thread-panel-layout";

it("leaves the page mounted and never creates a second Agent sidebar", () => {
  render(<AgentThreadPanelLayout><input aria-label="Page draft" /></AgentThreadPanelLayout>);
  const input = screen.getByRole("textbox", { name: "Page draft" });
  fireEvent.change(input, { target: { value: "unsaved" } });
  act(() => useAgentThreadPanelStore.getState().openPanel({ workspaceId: "workspace-1", taskId: "task-1", routePath: "/acme/issues" }));
  expect(screen.queryByRole("complementary")).toBeNull();
  expect(screen.queryByRole("separator")).toBeNull();
  expect(screen.getByRole("textbox", { name: "Page draft" })).toBe(input);
  expect(input).toHaveValue("unsaved");
  act(() => useAgentThreadPanelStore.getState().closePanel());
});
