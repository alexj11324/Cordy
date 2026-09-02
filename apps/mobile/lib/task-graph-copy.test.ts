import { describe, expect, it } from "vitest";
import {
  getTaskGraphCopy,
  getTaskGraphStateLabel,
  TASK_GRAPH_COPY_LOCALES,
} from "./task-graph-copy";

describe("task graph copy", () => {
  it("covers every string in all four product locales", () => {
    expect(TASK_GRAPH_COPY_LOCALES).toEqual(["en", "zh-Hans", "ja", "ko"]);
    for (const locale of TASK_GRAPH_COPY_LOCALES) {
      const copy = getTaskGraphCopy(locale);
      for (const [key, value] of Object.entries(copy)) {
        if (typeof value === "function") continue;
        expect(value.trim(), `${locale}.${key}`).not.toBe("");
      }
    }
  });

  it("produces non-empty interpolated strings in every locale", () => {
    for (const locale of TASK_GRAPH_COPY_LOCALES) {
      const copy = getTaskGraphCopy(locale);
      expect(copy.activePlans(3)).toContain("3");
      expect(copy.wave(2)).toContain("2");
      expect(copy.prerequisites(1, 4)).toContain("1");
      expect(copy.prerequisites(1, 4)).toContain("4");
      expect(copy.planLabel("abc12345")).toContain("abc12345");
      expect(copy.openNode("MUL-1")).toContain("MUL-1");
      expect(copy.loadFailed("boom")).toContain("boom");
      expect(copy.attentionRequired("gate")).toContain("gate");
      expect(
        copy.totals({ total: 9, ready: 2, running: 3, blocked: 4 }),
      ).toContain("9");
    }
  });

  it("normalizes regional and underscored account languages", () => {
    expect(getTaskGraphCopy("zh-CN").title).toBe("依赖图");
    expect(getTaskGraphCopy("ja_JP").title).toBe("依存グラフ");
    expect(getTaskGraphCopy("ko-KR").title).toBe("의존성 그래프");
    expect(getTaskGraphCopy("  JA  ").title).toBe("依存グラフ");
    expect(getTaskGraphCopy(null).title).toBe("Dependency Graph");
    expect(getTaskGraphCopy("fr").title).toBe("Dependency Graph");
  });

  it("translates the five known readiness states", () => {
    const copy = getTaskGraphCopy("zh-Hans");
    expect(getTaskGraphStateLabel(copy, "ready")).toBe("就绪");
    expect(getTaskGraphStateLabel(copy, "running")).toBe("执行中");
    expect(getTaskGraphStateLabel(copy, "blocked")).toBe("被阻塞");
    expect(getTaskGraphStateLabel(copy, "done")).toBe("已完成");
    expect(getTaskGraphStateLabel(copy, "cancelled")).toBe("已取消");
    expect(getTaskGraphStateLabel(copy, "todo")).toBe("待办");
  });

  it("passes an unknown state through instead of mislabelling it", () => {
    // Readiness state is an open string — a workspace catalog key this build
    // has never seen must render as itself, not as "Todo".
    const copy = getTaskGraphCopy("zh-Hans");
    expect(getTaskGraphStateLabel(copy, "awaiting_vendor")).toBe(
      "awaiting_vendor",
    );
    expect(getTaskGraphStateLabel(copy, "")).toBe("待办");
  });
});
