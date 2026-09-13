// @vitest-environment jsdom

import type { ReactNode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { screen } from "@testing-library/react";
import { renderWithI18n } from "../../test/i18n";
import enSettings from "../../locales/en/settings.json";

const data = vi.hoisted(() => ({
  skills: [] as Array<{ id: string; name: string; description: string }>,
  isLoading: false,
}));

vi.mock("@tanstack/react-query", () => ({
  useQuery: () => ({ data: data.skills, isLoading: data.isLoading }),
}));

vi.mock("@orvilo/core/workspace/queries", () => ({
  skillListOptions: () => ({ queryKey: ["workspaces", "workspace-1", "skills"] }),
}));

vi.mock("@orvilo/core/paths", () => ({
  useCurrentWorkspace: () => ({ id: "workspace-1", name: "Acme", slug: "acme" }),
  useWorkspacePaths: () => ({ skills: () => "/acme/skills" }),
}));

vi.mock("../../navigation", () => ({
  AppLink: ({
    href,
    children,
  }: {
    href: string;
    children: ReactNode;
  }) => <a href={href}>{children}</a>,
}));

import { SkillsTab } from "./skills-tab";

// `{ lobe: true }` is required: the tab is Lobe now, and the helper defaults the
// bridge off. Async because the bridge is lazy — until its module resolves the
// tree is a `Suspense` fallback of `null`, and the lede is the first thing
// every path renders.
async function renderTab() {
  const result = renderWithI18n(<SkillsTab />, { lobe: true });
  await screen.findByText(enSettings.skills.description);
  return result;
}

describe("SkillsTab", () => {
  beforeEach(() => {
    data.isLoading = false;
    data.skills = [];
  });

  it("explains that workspace skills are shared by every agent", async () => {
    await renderTab();
    expect(screen.getByText(enSettings.skills.description)).toBeInTheDocument();
    // The panel's own action. `SettingsTab`'s nested branch returned before it
    // read `action`, so this button never rendered inside the settings dialog —
    // the panel had zero buttons. It lives on the group's `extra` now.
    expect(
      screen.getByRole("link", { name: enSettings.skills.open_library }),
    ).toHaveAttribute("href", "/acme/skills");
  });

  it("lists library skills without a per-agent assignment control", async () => {
    data.skills = [
      { id: "sk-1", name: "review-pr", description: "Review a patch" },
    ];
    await renderTab();
    expect(screen.getByText("review-pr")).toBeInTheDocument();
    expect(screen.getByText("Review a patch")).toBeInTheDocument();
    expect(screen.queryByRole("checkbox")).toBeNull();
  });

  it("shows an empty library that still points at the Skills page", async () => {
    await renderTab();
    expect(screen.getByText(enSettings.skills.empty_title)).toBeInTheDocument();
    expect(
      screen.getByText(enSettings.skills.empty_description),
    ).toBeInTheDocument();
  });
});
