import { StrictMode, type ReactNode } from "react";
import { fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { BreadcrumbHeader } from "./breadcrumb-header";
import { ShellBreadcrumb } from "./shell-breadcrumb";
import { ShellHeaderActionsSlot, ShellHeaderProvider } from "./shell-header";

vi.mock("../navigation", () => ({
  AppLink: ({ href, children }: { href: string; children: ReactNode }) => <a href={href}>{children}</a>,
  useNavigation: () => ({ pathname: "/dev/automations/new", searchParams: new URLSearchParams() }),
}));
vi.mock("@orvilo/core/paths", () => ({
  useCurrentWorkspace: () => ({ name: "Dev", slug: "dev" }),
  useWorkspaceSlug: () => "dev",
  paths: { workspace: () => ({ issues: () => "/dev/issues" }) },
}));
vi.mock("./tab-presentation", () => ({ useTabPresentation: () => ({ title: "Automations" }) }));

function Shell({ children }: { children?: ReactNode }) {
  return (
    <StrictMode>
      <ShellHeaderProvider>
        <header data-testid="shell">
          <ShellBreadcrumb />
          <ShellHeaderActionsSlot />
        </header>
        <main>{children}</main>
      </ShellHeaderProvider>
    </StrictMode>
  );
}

const segments = [{ href: "/dev/automations", label: "Automations" }];

describe("BreadcrumbHeader shell integration", () => {
  it("uses one shell row with ancestor links and working page actions", () => {
    const save = vi.fn();
    render(<Shell><BreadcrumbHeader segments={segments} leaf="New automation" actions={<button onClick={save}>Create</button>} /></Shell>);
    const shell = within(screen.getByTestId("shell"));
    expect(shell.getByRole("link", { name: "Dev" })).toHaveAttribute("href", "/dev/issues");
    expect(shell.getByRole("link", { name: "Automations" })).toHaveAttribute("href", "/dev/automations");
    expect(screen.getAllByText("Automations")).toHaveLength(1);
    expect(shell.getByText("New automation")).toBeInTheDocument();
    expect(screen.getByRole("main")).toBeEmptyDOMElement();
    fireEvent.click(shell.getByRole("button", { name: "Create" }));
    expect(save).toHaveBeenCalledOnce();
  });

  it("updates the leaf and restores the route title and actions on navigation", () => {
    const { rerender } = render(<Shell><BreadcrumbHeader segments={segments} leaf="Draft" actions={<button>Create</button>} /></Shell>);
    rerender(<Shell><BreadcrumbHeader segments={segments} leaf="Renamed" /></Shell>);
    expect(within(screen.getByTestId("shell")).getByText("Renamed")).toBeInTheDocument();
    expect(screen.queryByText("Draft")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Create" })).not.toBeInTheDocument();
    rerender(<Shell />);
    expect(screen.getByRole("heading", { name: "Automations" })).toBeInTheDocument();
    expect(screen.queryByText("Renamed")).not.toBeInTheDocument();
  });

  it("keeps an embedded pane's breadcrumb and actions inside the pane", () => {
    render(<Shell><BreadcrumbHeader inline segments={segments} leaf="Embedded issue" actions={<button>Done</button>} /></Shell>);
    expect(screen.getByRole("heading", { name: "Automations" })).toBeInTheDocument();
    expect(within(screen.getByRole("main")).getByText("Embedded issue")).toBeInTheDocument();
    expect(within(screen.getByRole("main")).getByRole("button", { name: "Done" })).toBeInTheDocument();
  });

  it("keeps custom leading controls and standalone headers in place", () => {
    const { rerender } = render(<Shell><BreadcrumbHeader leading={<button>Back</button>} segments={segments} leaf="Issue" /></Shell>);
    expect(within(screen.getByRole("main")).getByRole("button", { name: "Back" })).toBeInTheDocument();
    rerender(<BreadcrumbHeader segments={segments} leaf="Standalone" actions={<button>Save</button>} />);
    expect(screen.getByText("Standalone")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Save" })).toBeInTheDocument();
  });
});
