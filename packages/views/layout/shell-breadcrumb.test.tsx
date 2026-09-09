import type { ReactNode } from "react";
import { screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { renderWithI18n } from "../test/i18n";

const workspace = vi.hoisted(() => ({
  current: { id: "ws-1", name: "Dev", slug: "acme" } as {
    id: string;
    name: string;
    slug: string;
  } | null,
}));

vi.mock("../navigation", () => ({
  AppLink: ({
    href,
    children,
  }: {
    href: string;
    children: ReactNode;
  }) => <a href={href}>{children}</a>,
  useNavigation: () => ({
    pathname: "/acme/teams",
    searchParams: new URLSearchParams(),
  }),
}));

vi.mock("@orvilo/core/paths", () => ({
  useCurrentWorkspace: () => workspace.current,
  useWorkspaceSlug: () => workspace.current?.slug ?? null,
  paths: {
    workspace: (slug: string) => ({
      issues: () => `/${slug}/issues`,
    }),
  },
}));

vi.mock("./tab-presentation", () => ({
  useTabPresentation: () => ({
    title: "Teams",
    visual: { kind: "icon", icon: "Users" },
  }),
}));

import { ShellBreadcrumb } from "./shell-breadcrumb";

describe("ShellBreadcrumb", () => {
  it("renders the workspace name and the current page title", () => {
    workspace.current = { id: "ws-1", name: "Dev", slug: "acme" };
    renderWithI18n(<ShellBreadcrumb />);

    expect(screen.getByRole("link", { name: "Dev" })).toHaveAttribute(
      "href",
      "/acme/issues",
    );
    expect(screen.getByText("Teams")).toBeInTheDocument();
  });

  it("renders nothing outside a workspace instead of throwing", () => {
    workspace.current = null;
    const { container } = renderWithI18n(<ShellBreadcrumb />);
    expect(container).toBeEmptyDOMElement();
  });
});
