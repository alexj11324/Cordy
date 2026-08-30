import { describe, expect, it } from "vitest";
import type { TFunction } from "i18next";
import { createI18n } from "@patchbay/core/i18n/react";
import enAutomations from "../../locales/en/automations.json";
import zhAutomations from "../../locales/zh-Hans/automations.json";
import { formatSchedulePartialFailureToast } from "./automation-dialog-toast";

// Contract test for the automation-dialog partial-success toast formatting.
//
// The dialog routes its partial-success branches through
// `formatSchedulePartialFailureToast`, so this test drives that exact
// helper rather than calling `t(...)` independently. That means a regression
// in either side — the JSON template (e.g. `{reason}` instead of `{{reason}}`)
// or the call-site variable name (e.g. `{ msg: ... }` instead of
// `{ reason: ... }`) — fails this test with the substring assertion.

describe("automation dialog partial-success toast", () => {
  const reason = "schedule conflict: 09:00 overlaps existing trigger";

  describe("en", () => {
    const i18n = createI18n("en", { en: { automations: enAutomations } });
    const t = i18n.getFixedT("en", "automations") as TFunction<"automations">;

    it("renders create partial-success with the server reason verbatim", () => {
      const rendered = formatSchedulePartialFailureToast(t, "create", reason);
      expect(rendered).toContain(reason);
      expect(rendered).not.toContain("{{");
      expect(rendered).not.toContain("{reason}");
    });

    it("renders update partial-success with the server reason verbatim", () => {
      const rendered = formatSchedulePartialFailureToast(t, "update", reason);
      expect(rendered).toContain(reason);
      expect(rendered).not.toContain("{{");
      expect(rendered).not.toContain("{reason}");
    });

    it("falls back to the no-reason create string when reason is null", () => {
      expect(formatSchedulePartialFailureToast(t, "create", null)).toBe(
        "Automation created, but schedule failed to save",
      );
    });

    it("falls back to the no-reason update string when reason is null", () => {
      expect(formatSchedulePartialFailureToast(t, "update", null)).toBe(
        "Automation updated, but schedule failed to save",
      );
    });
  });

  describe("zh-Hans", () => {
    const i18n = createI18n("zh-Hans", {
      "zh-Hans": { automations: zhAutomations },
      en: { automations: enAutomations },
    });
    const t = i18n.getFixedT("zh-Hans", "automations") as TFunction<"automations">;

    it("renders create partial-success with the server reason verbatim", () => {
      const rendered = formatSchedulePartialFailureToast(t, "create", reason);
      expect(rendered).toContain(reason);
      expect(rendered).not.toContain("{{");
      expect(rendered).not.toContain("{reason}");
    });

    it("renders update partial-success with the server reason verbatim", () => {
      const rendered = formatSchedulePartialFailureToast(t, "update", reason);
      expect(rendered).toContain(reason);
      expect(rendered).not.toContain("{{");
      expect(rendered).not.toContain("{reason}");
    });
  });
});
