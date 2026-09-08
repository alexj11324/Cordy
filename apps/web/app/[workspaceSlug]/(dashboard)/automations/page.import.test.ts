// @vitest-environment node
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const dir = dirname(fileURLToPath(import.meta.url));

describe("web automations pages share packages/views", () => {
  it("list and detail routes only re-export the shared views components", () => {
    const list = readFileSync(resolve(dir, "page.tsx"), "utf8");
    const detail = readFileSync(resolve(dir, "[id]/page.tsx"), "utf8");

    expect(list).toContain('from "@orvilo/views/automations/components"');
    expect(detail).toContain('from "@orvilo/views/automations/components"');
    expect(list).toContain("<AutomationsPage");
    expect(detail).toContain("<AutomationDetailPage");
    expect(list).not.toMatch(/function AutomationsPage/);
    expect(detail).not.toMatch(/function AutomationDetailPage/);
  });
});
