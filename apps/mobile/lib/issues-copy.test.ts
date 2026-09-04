import { describe, expect, it } from "vitest";
import { PRODUCT_LOCALES } from "./locale";
import { getIssuesCopy } from "./issues-copy";

describe("mobile issue list copy", () => {
  it("has complete non-empty copy for every supported locale", () => {
    for (const locale of PRODUCT_LOCALES) {
      const copy = getIssuesCopy(locale);
      expect(copy.title, `${locale}:title`).not.toBe("");
      expect(copy.myTitle, `${locale}:myTitle`).not.toBe("");
      expect(copy.filter, `${locale}:filter`).not.toBe("");
      expect(copy.retry, `${locale}:retry`).not.toBe("");
      expect(copy.scopes.all, `${locale}:scopes.all`).not.toBe("");
      expect(copy.scopes.members, `${locale}:scopes.members`).not.toBe("");
      expect(copy.scopes.agents, `${locale}:scopes.agents`).not.toBe("");
      expect(copy.empty.workspace, `${locale}:empty.workspace`).not.toBe("");
      expect(copy.empty.agents, `${locale}:empty.agents`).not.toBe("");
      expect(copy.priority.urgent, `${locale}:priority.urgent`).not.toBe("");
    }
  });

  it("normalizes account language tags", () => {
    expect(getIssuesCopy("zh-CN").scopes.members).toBe("成员");
    expect(getIssuesCopy("ja_JP").retry).toBe("再試行");
    expect(getIssuesCopy("ko-KR").scopes.agents).toBe("에이전트");
  });
});
