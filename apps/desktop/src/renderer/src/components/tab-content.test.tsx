import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createMemoryRouter } from "react-router-dom";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { getActiveTab, useTabStore } from "@/stores/tab-store";
import { __resetTabCoordinatorForTests, getAppRouter } from "@/platform/tab-coordinator";
import { TabContent } from "./tab-content";

vi.mock("@/routes", () => ({
  createAppRouter: () => createMemoryRouter([
    { path: "/", element: <div>Parked router</div> },
    { path: "/acme/issues", element: <h1>Tasks</h1> },
    { path: "/acme/work-products", element: <h1>Work products</h1> },
    { path: "/acme/projects", element: <h1>Projects</h1> },
  ]),
}));

beforeEach(() => {
  __resetTabCoordinatorForTests();
  useTabStore.getState().reset();
  useTabStore.getState().switchWorkspace("acme");
});
afterEach(() => {
  cleanup();
  __resetTabCoordinatorForTests();
});

function mount() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const tree = () => <QueryClientProvider client={client}><TabContent /></QueryClientProvider>;
  return { ...render(tree()), tree };
}

it("keeps sidebar navigation, tab switching and closing the last tab in sync", async () => {
  mount();
  expect(await screen.findByRole("heading", { name: "Tasks" })).toBeInTheDocument();
  act(() => useTabStore.getState().navigateActiveSession("/acme/work-products"));
  expect(await screen.findByRole("heading", { name: "Work products" })).toBeInTheDocument();
  const first = getActiveTab(useTabStore.getState())!.id;
  let second = "";
  act(() => { second = useTabStore.getState().openTab("/acme/projects", "Projects", { activate: true }); });
  expect(await screen.findByRole("heading", { name: "Projects" })).toBeInTheDocument();
  act(() => useTabStore.getState().setActiveTab(first));
  expect(await screen.findByRole("heading", { name: "Work products" })).toBeInTheDocument();
  act(() => useTabStore.getState().closeTab(first));
  expect(await screen.findByRole("heading", { name: "Projects" })).toBeInTheDocument();
  act(() => useTabStore.getState().closeTab(second));
  expect(await screen.findByRole("heading", { name: "Tasks" })).toBeInTheDocument();
  expect(getAppRouter().state.location.pathname).toBe(getActiveTab(useTabStore.getState())!.url);
});

it("reconnects a replacement coordinator while React keeps the tab host mounted", async () => {
  const { rerender, tree } = mount();
  expect(await screen.findByRole("heading", { name: "Tasks" })).toBeInTheDocument();
  const oldRouter = getAppRouter();
  // A hot update replaces module-owned state while preserving React hooks.
  act(() => __resetTabCoordinatorForTests());
  rerender(tree());
  expect(getAppRouter()).not.toBe(oldRouter);
  act(() => useTabStore.getState().navigateActiveSession("/acme/work-products"));
  await waitFor(() => expect(getAppRouter().state.location.pathname).toBe("/acme/work-products"));
  expect(await screen.findByRole("heading", { name: "Work products" })).toBeInTheDocument();
  act(() => useTabStore.getState().closeTab(getActiveTab(useTabStore.getState())!.id));
  expect(await screen.findByRole("heading", { name: "Tasks" })).toBeInTheDocument();
});

it("shows the latest session immediately when its router instance is replaced", async () => {
  const { rerender, tree } = mount();
  expect(await screen.findByRole("heading", { name: "Tasks" })).toBeInTheDocument();
  act(() => {
    __resetTabCoordinatorForTests();
    useTabStore.getState().navigateActiveSession("/acme/work-products");
  });
  rerender(tree());
  expect(getAppRouter().state.location.pathname).toBe("/acme/work-products");
  expect(screen.getByRole("heading", { name: "Work products" })).toBeInTheDocument();
});
