// @vitest-environment node
import { describe, expect, it } from "vitest";
import {
  AUTOMATION_TRIGGER_PRESETS,
  AUTOMATION_TRIGGER_SOURCES,
  automationTriggerPreset,
  isNativeAutomationProvider,
  parseAutomationTools,
  parseAutomationTriggerConfig,
  presetsForSource,
  searchTriggerCatalog,
  settingsPathForTriggerProvider,
} from "./trigger-catalog";

describe("automation trigger catalog", () => {
  it("preserves GitHub repository and event-state conditions", () => {
    expect(parseAutomationTriggerConfig({ repository: "acme/app", repositories: ["acme/app", "acme/api"],
      author_scope: "specific", author_logins: ["octocat"], review_state: "approved",
      thread_state: "resolved", conclusion: "success" })).toMatchObject({
      repository: "acme/app", repositories: ["acme/app", "acme/api"],
      author_scope: "specific", author_logins: ["octocat"], review_state: "approved",
      thread_state: "resolved", conclusion: "success",
    });
  });

  it("preserves identity-backed Slack and Linear conditions", () => {
    expect(parseAutomationTriggerConfig({
      installation_id: "installation-1",
      channel: "C123",
      sender_scope: "authenticated",
      ignore_thread_replies: false,
      keyword: "incident",
      completion_reaction: "white_check_mark",
      team_id: "team-1",
      project_id: "project-1",
      status_id: "state-1",
    })).toMatchObject({
      installation_id: "installation-1",
      channel: "C123",
      sender_scope: "authenticated",
      ignore_thread_replies: false,
      keyword: "incident",
      completion_reaction: "white_check_mark",
      team_id: "team-1",
      project_id: "project-1",
      status_id: "state-1",
    });
  });
  it("keeps webhook as a last-class source, not the default mental model", () => {
    expect(AUTOMATION_TRIGGER_SOURCES.map((source) => source.id)).toEqual([
      "scheduled",
      "github",
      "slack",
      "linear",
      "webhook",
    ]);
    expect(AUTOMATION_TRIGGER_SOURCES.at(-1)?.id).toBe("webhook");
  });

  it("exposes Cursor-shaped GitHub / Slack / Linear presets", () => {
    expect(presetsForSource("github").map((preset) => preset.id)).toContain(
      "github.pull_request.opened",
    );
    expect(presetsForSource("slack").map((preset) => preset.id)).toEqual([
      "slack.message",
      "slack.reaction",
      "slack.channel_created",
    ]);
    expect(presetsForSource("linear").map((preset) => preset.id)).toEqual([
      "linear.issue.created",
      "linear.issue.status_changed",
      "linear.cycle.ended",
    ]);
  });

  it("looks up presets by stable id and ignores unknown values", () => {
    expect(automationTriggerPreset("github.ci_completed")?.provider).toBe("github");
    expect(automationTriggerPreset("not-a-preset")).toBeNull();
    expect(automationTriggerPreset(undefined)).toBeNull();
  });

  it("filters the nested add-trigger menu by query", () => {
    const hit = searchTriggerCatalog("review comment");
    expect(hit.presets.some((preset) => preset.id === "github.pull_request.review_comment")).toBe(
      true,
    );
    expect(hit.sources.some((source) => source.id === "github")).toBe(true);
  });

  it("treats github/slack/linear as native providers that do not mint a URL", () => {
    expect(isNativeAutomationProvider("github")).toBe(true);
    expect(isNativeAutomationProvider("generic")).toBe(false);
    expect(AUTOMATION_TRIGGER_PRESETS.every((preset) => preset.id.length > 0)).toBe(true);
  });

  it("parses additive tools and trigger config defensively", () => {
    expect(parseAutomationTools({ memories: { enabled: true }, extra: 1 })).toEqual({
      memories: { enabled: true },
    });
    expect(parseAutomationTriggerConfig({ channel: "#bugs", on_failure: true })).toEqual({
      channel: "#bugs",
      keyword: undefined,
      regex: undefined,
      emoji: undefined,
      branch: undefined,
      label: undefined,
      on_failure: true,
    });
    expect(settingsPathForTriggerProvider("/acme/settings", "github")).toBe(
      "/acme/settings?tab=github",
    );
  });
});
