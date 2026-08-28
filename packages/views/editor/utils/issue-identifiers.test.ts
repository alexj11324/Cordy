import { describe, expect, it } from "vitest";
import {
  preprocessIssueIdentifiers,
  isIssueIdentifier,
} from "@cordy/ui/markdown";

/**
 * Pure detector for the Linear-style issue-identifier autolink. Lives in
 * @cordy/ui/markdown (no test runner there), exercised here where views'
 * vitest can reach it.
 */
describe("preprocessIssueIdentifiers", () => {
  it("rewrites a bare identifier into a canonical mention link", () => {
    expect(preprocessIssueIdentifiers("Related to PB-1745")).toBe(
      "Related to [PB-1745](mention://issue/PB-1745)",
    );
  });

  it("rewrites multiple identifiers in one string", () => {
    expect(preprocessIssueIdentifiers("Created TES-1 and PB-2")).toBe(
      "Created [TES-1](mention://issue/TES-1) and [PB-2](mention://issue/PB-2)",
    );
  });

  it("links an identifier at a sentence end (trailing dot + space)", () => {
    expect(preprocessIssueIdentifiers("See PB-1. Done.")).toBe(
      "See [PB-1](mention://issue/PB-1). Done.",
    );
  });

  it("links identifiers wrapped in prose punctuation", () => {
    expect(preprocessIssueIdentifiers("(PB-1) and [PB-2]")).toContain(
      "([PB-1](mention://issue/PB-1))",
    );
  });

  // --- skip: code -------------------------------------------------------
  it("skips identifiers inside inline code", () => {
    expect(preprocessIssueIdentifiers("use `PB-1` here")).toBe(
      "use `PB-1` here",
    );
  });

  it("skips identifiers inside fenced code blocks", () => {
    const input = "```\nPB-1 in code\n```";
    expect(preprocessIssueIdentifiers(input)).toBe(input);
  });

  // --- skip: existing links / mentions ----------------------------------
  it("does not double-process an existing mention link", () => {
    const input = "[PB-1](mention://issue/00000000-0000-0000-0000-000000000001)";
    expect(preprocessIssueIdentifiers(input)).toBe(input);
  });

  it("skips an identifier used as a markdown link label", () => {
    const input = "[PB-1](https://example.com/x)";
    expect(preprocessIssueIdentifiers(input)).toBe(input);
  });

  // --- skip: urls / filenames / paths -----------------------------------
  it("skips an identifier inside a URL", () => {
    const input = "https://example.com/board/PB-1";
    expect(preprocessIssueIdentifiers(input)).toBe(input);
  });

  it("skips a filename token like ABC-123.ts", () => {
    const input = "open ABC-123.ts now";
    expect(preprocessIssueIdentifiers(input)).toBe(input);
  });

  it("skips a path segment like FOO-1/bar", () => {
    const input = "path FOO-1/bar/baz";
    expect(preprocessIssueIdentifiers(input)).toBe(input);
  });

  // --- non-matches ------------------------------------------------------
  it("ignores lowercase tokens", () => {
    const input = "some-word-1 and pb-1";
    expect(preprocessIssueIdentifiers(input)).toBe(input);
  });

  it("ignores a token embedded in a larger word", () => {
    const input = "XPB-1A stays";
    expect(preprocessIssueIdentifiers(input)).toBe(input);
  });

  it("returns input unchanged when no candidates exist", () => {
    const input = "plain text with no identifiers";
    expect(preprocessIssueIdentifiers(input)).toBe(input);
  });
});

describe("isIssueIdentifier", () => {
  it("accepts a bare identifier", () => {
    expect(isIssueIdentifier("PB-1745")).toBe(true);
    expect(isIssueIdentifier("TES-1")).toBe(true);
  });

  it("rejects a UUID (so real mentions are not treated as identifiers)", () => {
    expect(isIssueIdentifier("00000000-0000-0000-0000-000000000001")).toBe(
      false,
    );
  });

  it("rejects lowercase and malformed tokens", () => {
    expect(isIssueIdentifier("pb-1")).toBe(false);
    expect(isIssueIdentifier("PB-")).toBe(false);
    expect(isIssueIdentifier("PB1")).toBe(false);
  });
});
