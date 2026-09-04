import { describe, expect, it } from "vitest";
import {
  formatIssueRoleCopy,
  getIssueRoleCopy,
  ISSUE_ROLE_COPY_LOCALES,
} from "./issue-role-copy";

describe("issue role copy", () => {
  it("provides every role-picker label in all four product locales", () => {
    expect(ISSUE_ROLE_COPY_LOCALES).toEqual(["en", "zh-Hans", "ja", "ko"]);
    for (const locale of ISSUE_ROLE_COPY_LOCALES) {
      for (const value of Object.values(getIssueRoleCopy(locale))) {
        expect(value.trim()).not.toBe("");
      }
    }
  });

  it("normalizes account language variants", () => {
    expect(getIssueRoleCopy("zh-CN").owner).toBe("负责人");
    expect(getIssueRoleCopy("ja-JP").reviewer).toBe("レビュー担当");
    expect(getIssueRoleCopy("ko-KR").executor).toBe("실행자");
    expect(getIssueRoleCopy("fr").owner).toBe("Owner");
  });

  it("keeps reviewer selection and review handoff wording distinct", () => {
    const copy = getIssueRoleCopy("en");
    expect(copy.reviewer).toBe("Reviewer");
    expect(copy.reviewHandoff).toBe("Review handoff");
    expect(copy.reviewerRequired).toContain("reviewer");
    expect(copy.reviewerMustDiffer).toContain("executor");
    expect(
      formatIssueRoleCopy(copy.reviewHandoffFromTo, {
        from: "Build Agent",
        to: "Alex",
      }),
    ).toBe("handed review from Build Agent to Alex");
    expect(formatIssueRoleCopy(copy.reviewerAssignedTo, { name: "Alex" })).toBe(
      "assigned reviewer to Alex",
    );
  });

  it("localizes review activity and inbox copy in every product language", () => {
    expect(
      formatIssueRoleCopy(getIssueRoleCopy("zh-CN").reviewRequestedFor, {
        name: "小林",
      }),
    ).toBe("已请求小林进行审核");
    expect(
      formatIssueRoleCopy(getIssueRoleCopy("ja-JP").reviewerChangedFromTo, {
        from: "A",
        to: "B",
      }),
    ).toBe("レビュー担当を A から B に変更しました");
    expect(getIssueRoleCopy("ko-KR").reviewerRemoved).toBe(
      "검토자를 제거했습니다",
    );
  });
});
