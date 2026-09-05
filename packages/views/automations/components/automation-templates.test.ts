// @vitest-environment node
import { describe, expect, it } from "vitest";
import enAutomations from "../../locales/en/automations.json";
import {
  AUTOMATION_TEMPLATE_IDS,
  AUTOMATION_TEMPLATES,
  TEMPLATE_ACTION_IDS,
  TEMPLATE_CATEGORIES,
  TEMPLATE_CATEGORY_IDS,
  TEMPLATE_TRIGGER_IDS,
} from "./automation-templates";

describe("automation template catalog", () => {
  it("exposes the six gallery categories in screenshot order", () => {
    expect(TEMPLATE_CATEGORIES.map((category) => category.id)).toEqual([
      ...TEMPLATE_CATEGORY_IDS,
    ]);
    expect(TEMPLATE_CATEGORY_IDS).toEqual([
      "popular",
      "code_review",
      "security",
      "incidents_triage",
      "data_research",
      "environment",
    ]);
  });

  it("defines every card in the gallery against a runbook", () => {
    const referenced = new Set<string>();
    for (const category of TEMPLATE_CATEGORIES) {
      expect(category.templateIds.length).toBeGreaterThan(0);
      for (const id of category.templateIds) {
        referenced.add(id);
        const template = AUTOMATION_TEMPLATES[id];
        expect(template.id).toBe(id);
        expect(template.prompt.length).toBeGreaterThan(80);
        expect(template.prompt).toMatch(/^# Goal/m);
        if (template.triggerKind === "schedule") {
          expect(template.schedule.time).toBeDefined();
          expect(template.schedule.days).toBeDefined();
        } else {
          expect(template.eventFilters.length).toBeGreaterThan(0);
        }
      }
    }
    expect([...referenced].sort()).toEqual([...AUTOMATION_TEMPLATE_IDS].sort());
  });

  it("keeps popular cards as the screenshot set", () => {
    expect(TEMPLATE_CATEGORIES[0]?.templateIds).toEqual([
      "find_critical_bugs",
      "scan_codebase_vulnerabilities",
      "generate_docs",
      "add_test_coverage",
    ]);
  });

  it("keeps catalog ids aligned with the English locale keys", () => {
    const templates = enAutomations.templates;
    const categories = enAutomations.template_categories;
    const triggers = enAutomations.template_flow.trigger;
    const actions = enAutomations.template_flow.action;
    expect(Object.keys(templates).sort()).toEqual(
      [...AUTOMATION_TEMPLATE_IDS].sort(),
    );
    expect(Object.keys(categories).sort()).toEqual(
      [...TEMPLATE_CATEGORY_IDS].sort(),
    );
    expect(Object.keys(triggers).sort()).toEqual(
      [...TEMPLATE_TRIGGER_IDS].sort(),
    );
    expect(Object.keys(actions).sort()).toEqual([...TEMPLATE_ACTION_IDS].sort());
  });
});
