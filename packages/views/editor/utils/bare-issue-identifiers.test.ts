import { describe, expect, it } from "vitest";
import { resolveBareIssueIdentifiersInMarkdown } from "./bare-issue-identifiers";

describe("resolveBareIssueIdentifiersInMarkdown", () => {
  it("rewrites a resolved identifier into a mention://issue UUID token", async () => {
    const next = await resolveBareIssueIdentifiersInMarkdown(
      "Related to PB-1745 please",
      async (identifier) =>
        identifier === "PB-1745"
          ? { id: "issue-uuid", identifier: "PB-1745" }
          : null,
    );
    expect(next).toBe("Related to [PB-1745](mention://issue/issue-uuid) please");
  });

  it("leaves unresolved identifiers as plain text", async () => {
    const source = "See PB-999";
    const next = await resolveBareIssueIdentifiersInMarkdown(source, async () => null);
    expect(next).toBe(source);
  });

  it("does not rewrite identifiers inside code or existing links", async () => {
    const source = "use `PB-1` and [PB-2](https://example.com/PB-2)";
    const next = await resolveBareIssueIdentifiersInMarkdown(
      source,
      async (identifier) => ({ id: `id-${identifier}`, identifier }),
    );
    expect(next).toBe(source);
  });
});
