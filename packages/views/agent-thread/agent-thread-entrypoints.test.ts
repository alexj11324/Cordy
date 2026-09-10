// @vitest-environment node

import { describe, expect, it } from "vitest";
import { existsSync, readFileSync, readdirSync, statSync } from "node:fs";
import { join, relative } from "node:path";

const VIEWS_ROOT = join(__dirname, "..");

const REMOVED_TRANSCRIPT_FILES = [
  "common/task-transcript/agent-transcript-dialog.tsx",
  "common/task-transcript/transcript-button.tsx",
  "common/task-transcript/build-steps.ts",
  "common/task-transcript/detail-surfaces.tsx",
  "common/task-transcript/diff-highlight.ts",
  "common/task-transcript/run-outcome.ts",
  "common/task-transcript/run-timeline.tsx",
  "common/task-transcript/task-transcript.css",
  "common/task-transcript/index.ts",
];

const AGENT_THREAD_ENTRYPOINTS = [
  "agents/components/tabs/activity-tab.tsx",
  "automations/components/automation-detail-page.tsx",
  "issues/components/execution-log-section.tsx",
];

function walk(dir: string, out: string[] = []): string[] {
  for (const entry of readdirSync(dir)) {
    if (entry === "node_modules" || entry.startsWith(".")) continue;
    const path = join(dir, entry);
    if (statSync(path).isDirectory()) walk(path, out);
    else if (/\.tsx?$/.test(path) && !/\.test\.tsx?$/.test(path)) out.push(path);
  }
  return out;
}

describe("Agent thread entrypoint boundary", () => {
  it("keeps the obsolete read-only transcript surface deleted", () => {
    const restoredFiles = REMOVED_TRANSCRIPT_FILES.filter((path) =>
      existsSync(join(VIEWS_ROOT, path)),
    );
    const packageManifest = readFileSync(
      join(VIEWS_ROOT, "package.json"),
      "utf8",
    );

    expect(restoredFiles).toEqual([]);
    expect(packageManifest).not.toMatch(/"\.\/common\/task-transcript"/);
  });

  it("routes every visible task entrypoint through AgentThreadButton", () => {
    const missing = AGENT_THREAD_ENTRYPOINTS.filter((path) =>
      !readFileSync(join(VIEWS_ROOT, path), "utf8").includes("AgentThreadButton"),
    );

    expect(missing).toEqual([]);
  });

  it("keeps the issue live-status chip free of a duplicate conversation entry", () => {
    const source = readFileSync(
      join(VIEWS_ROOT, "issues/components/issue-agent-header-chip.tsx"),
      "utf8",
    );

    expect(source).not.toContain("AgentThreadButton");
    expect(source).toContain("showConversationAction={false}");
  });

  it("does not reference the deleted transcript components from product code", () => {
    const offenders = walk(VIEWS_ROOT)
      .map((path) => ({
        path: relative(VIEWS_ROOT, path),
        source: readFileSync(path, "utf8"),
      }))
      .filter(({ source }) =>
        /AgentTranscriptDialog|TranscriptButton|agent-transcript-dialog|transcript-button/.test(source),
      )
      .map(({ path }) => path);

    expect(offenders).toEqual([]);
  });

  it("hosts task conversations in the right panel without a dialog fallback", () => {
    const components = join(VIEWS_ROOT, "agent-thread/components");
    expect(existsSync(join(components, "task-agent-thread-dialog.tsx"))).toBe(false);
    expect(readFileSync(join(components, "agent-thread-surface.tsx"), "utf8"))
      .not.toContain("components/ui/dialog");
    expect(readFileSync(join(components, "agent-thread-button.tsx"), "utf8"))
      .not.toContain("TaskAgentThreadDialog");
  });
});
