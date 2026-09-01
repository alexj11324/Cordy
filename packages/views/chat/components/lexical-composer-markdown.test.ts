import { describe, expect, it } from "vitest";
import {
  mentionChipLabel,
  serializeComposerMention,
} from "./lexical-composer-markdown";

describe("lexical composer markdown", () => {
  it("serializes member mentions in the Patchbay token format", () => {
    expect(
      serializeComposerMention({
        label: "@Alice",
        metadata: { id: "user-1", type: "member", label: "Alice" },
      }),
    ).toBe("[@Alice](mention://member/user-1)");
  });

  it("omits the @ prefix for issue and project mentions", () => {
    expect(
      serializeComposerMention({
        label: "PB-12",
        metadata: { id: "issue-1", type: "issue", label: "PB-12" },
      }),
    ).toBe("[PB-12](mention://issue/issue-1)");
  });

  it("serializes slash skills as slash:// tokens", () => {
    expect(
      serializeComposerMention({
        label: "/deploy",
        metadata: { id: "skill-1", type: "skill", label: "deploy" },
      }),
    ).toBe("[/deploy](slash://skill/skill-1)");
  });

  it("escapes markdown-significant characters in labels", () => {
    expect(
      serializeComposerMention({
        label: "@David[TF]",
        metadata: { id: "agent-1", type: "agent", label: "David[TF]" },
      }),
    ).toBe("[@David\\[TF\\]](mention://agent/agent-1)");
  });

  it("builds chip labels that match the Tiptap mention / slash pills", () => {
    expect(mentionChipLabel("member", "Alice")).toBe("@Alice");
    expect(mentionChipLabel("issue", "PB-12")).toBe("PB-12");
    expect(mentionChipLabel("skill", "deploy")).toBe("/deploy");
  });
});
