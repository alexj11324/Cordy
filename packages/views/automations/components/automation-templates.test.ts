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
  templateTriggerPreset,
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

  // The server normalizes provider deliveries to `<provider>.<event>[.<action>]`
  // (see inferProviderEvent in server/internal/handler/automation_webhook.go).
  // A seeded filter that cannot match its own provider makes the template look
  // installed while every delivery is recorded as `event_filtered`.
  it("seeds event filters the intended provider can actually satisfy", () => {
    const filtersFor = (id: (typeof AUTOMATION_TEMPLATE_IDS)[number]) => {
      const template = AUTOMATION_TEMPLATES[id];
      return template.triggerKind === "webhook" ? template.eventFilters : [];
    };
    // Linear: `{"type":"Issue","action":"create"}` -> linear.issue.create
    for (const id of [
      "triage_linear_issues",
      "remediate_dependency_vulnerabilities",
    ] as const) {
      expect(filtersFor(id)).toEqual([{ event: "issue", actions: ["create"] }]);
    }
    // Slack: `{"event":{"type":"message"}}` -> slack.message
    for (const id of ["fix_bugs_reported_in_slack", "product_faq"] as const) {
      expect(filtersFor(id)).toEqual([{ event: "message" }]);
    }
    // PagerDuty: `{"event":{"event_type":"incident.triggered"}}`
    expect(filtersFor("investigate_pagerduty_incidents")).toEqual([
      { event: "incident" },
    ]);
    // Sentry names the resource in a header: `issue` or `error`.
    expect(filtersFor("investigate_sentry_issues")).toEqual([
      { event: "error" },
      { event: "issue" },
    ]);
  });

  it("maps webhook templates onto native catalog presets", () => {
    expect(templateTriggerPreset("pr_opened")).toBe("github.pull_request.opened");
    expect(templateTriggerPreset("pr_pushed")).toBe("github.pull_request.pushed");
    expect(templateTriggerPreset("pr_review_comment")).toBe(
      "github.pull_request.review_comment",
    );
    expect(templateTriggerPreset("new_message_in_channel")).toBe("slack.message");
    expect(templateTriggerPreset("workflow_run_completed")).toBe(
      "github.workflow_run.completed",
    );
    expect(templateTriggerPreset("checks_completed")).toBe("github.ci_completed");
    expect(templateTriggerPreset("issue_created")).toBe("linear.issue.created");
    expect(templateTriggerPreset("scheduled")).toBeNull();
    expect(templateTriggerPreset("incident_triggered")).toBeNull();
    expect(templateTriggerPreset("sentry_issue_event")).toBeNull();
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
