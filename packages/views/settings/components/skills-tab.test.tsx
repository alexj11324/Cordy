// @vitest-environment jsdom

import type { ReactNode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { I18nProvider } from "@orvilo/core/i18n/react";
import enCommon from "../../locales/en/common.json";
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

const TEST_RESOURCES = {
  en: { common: enCommon, settings: enSettings },
};

function renderTab() {
  return render(
    <I18nProvider locale="en" resources={TEST_RESOURCES}>
      <SkillsTab />
    </I18nProvider>,
  );
}

describe("SkillsTab", () => {
  beforeEach(() => {
    data.isLoading = false;
    data.skills = [];
  });

  it("explains that workspace skills are shared by every agent", () => {
    renderTab();
    expect(screen.getByText(enSettings.skills.description)).toBeInTheDocument();
    expect(
      screen.getByRole("link", { name: enSettings.skills.open_library }),
    ).toHaveAttribute("href", "/acme/skills");
  });

  it("lists library skills without a per-agent assignment control", () => {
    data.skills = [
      { id: "sk-1", name: "review-pr", description: "Review a patch" },
    ];
    renderTab();
    expect(screen.getByText("review-pr")).toBeInTheDocument();
    expect(screen.getByText("Review a patch")).toBeInTheDocument();
    expect(screen.queryByRole("checkbox")).toBeNull();
  });

  it("shows an empty library that still points at the Skills page", () => {
    renderTab();
    expect(screen.getByText(enSettings.skills.empty_title)).toBeInTheDocument();
    expect(
      screen.getByText(enSettings.skills.empty_description),
    ).toBeInTheDocument();
  });
});
