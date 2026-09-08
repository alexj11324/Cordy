// @vitest-environment node
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const dir = dirname(fileURLToPath(import.meta.url));
const routes = resolve(dir, "../routes.tsx");
const detail = resolve(dir, "automation-detail-page.tsx");

describe("desktop automations pages share packages/views", () => {
  it("list route imports the shared AutomationsPage", () => {
    const source = readFileSync(routes, "utf8");
    expect(source).toContain('from "@orvilo/views/automations/components"');
    expect(source).toContain("AutomationsPage");
    expect(source).toMatch(/path:\s*"automations"/);
  });

  it("detail wrapper only adds a document title around the shared page", () => {
    const source = readFileSync(detail, "utf8");
    expect(source).toContain('from "@orvilo/views/automations/components"');
    expect(source).toContain("<AutomationDetail automationId={id}");
    expect(source).not.toMatch(/section_properties/);
  });
});
