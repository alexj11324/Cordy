import { describe, expect, it } from "vitest";
import {
  decodeProjectBuilderInput,
  encodeProjectBuilderInput,
  mergeProjectBuilderDraft,
  parseProjectBuilderDraft,
  stripProjectBuilderDraft,
  type ProjectBuilderCatalogs,
  type ProjectBuilderDraft,
} from "./project-builder-draft";

const draft: ProjectBuilderDraft = {
  title: "Existing project",
  summary: "A concise summary",
  description: "Keep this context",
  icon: "📁",
  status: "planned",
  priority: "medium",
  lead_type: null,
  lead_id: null,
  start_date: null,
  due_date: null,
  member_ids: ["member-1"],
  label_ids: ["label-1"],
  dependency_ids: [],
};

const catalogs: ProjectBuilderCatalogs = {
  members: [{ id: "member-1", name: "Alex" }],
  agents: [{ id: "agent-1", name: "Planner" }],
  labels: [{ id: "label-1", name: "Roadmap" }],
  projects: [{ id: "project-1", name: "Existing project" }],
};

describe("project builder draft protocol", () => {
  it("uses the latest complete draft block and hides it from transcript copy", () => {
    const content = [
      "I started with a rough idea.",
      '<project_draft>{"title":"First"}</project_draft>',
      "Here is the refined version.",
      '<project_draft>{"title":"Second","description":"A\\nline"}</project_draft>',
    ].join("\n");

    expect(parseProjectBuilderDraft(content)).toEqual({
      title: "Second",
      description: "A\nline",
    });
    expect(stripProjectBuilderDraft(content)).toBe(
      "I started with a rough idea.\nHere is the refined version.",
    );
  });

  it("repairs literal control characters inside a model JSON string", () => {
    const content = '<project_draft>{"description":"first\nsecond"}</project_draft>';
    expect(parseProjectBuilderDraft(content)).toEqual({
      description: "first\nsecond",
    });
  });

  it("filters catalog ids and malformed metadata before applying a proposal", () => {
    const result = mergeProjectBuilderDraft(
      draft,
      {
        title: "Refined project",
        status: "in_progress",
        priority: "urgent",
        lead_type: "agent",
        lead_id: "agent-1",
        member_ids: ["member-1", "forged-member"],
        label_ids: ["forged-label"],
        dependency_ids: ["project-1", "forged-project"],
        start_date: "2026-09-10",
        due_date: "not-a-date",
      },
      catalogs,
    );

    expect(result).toMatchObject({
      title: "Refined project",
      status: "in_progress",
      priority: "urgent",
      lead_type: "agent",
      lead_id: "agent-1",
      member_ids: ["member-1"],
      label_ids: [],
      dependency_ids: ["project-1"],
      start_date: "2026-09-10",
      due_date: null,
    });
  });

  it("round-trips the user request while keeping the assistant context opaque", () => {
    const encoded = encodeProjectBuilderInput("Make it launch ready", draft, catalogs);

    expect(decodeProjectBuilderInput(encoded)).toBe("Make it launch ready");
    expect(encoded).toContain('"project_builder_instructions"');
    expect(encoded).toContain('"current_draft"');
    expect(encoded).toContain('"available_project_labels"');
    expect(decodeProjectBuilderInput("ordinary message")).toBe("ordinary message");
  });
});
