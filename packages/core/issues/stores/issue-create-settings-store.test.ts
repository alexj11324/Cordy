import { beforeEach, describe, expect, it } from "vitest";
import {
  DEFAULT_MANUAL_CREATE_FIELDS,
  DEFAULT_QUICK_CREATE_FIELDS,
  MANUAL_CREATE_FIELDS,
  migrateIssueCreateSettings,
  normalizeIssueCreateFields,
  useIssueCreateSettingsStore,
} from "./issue-create-settings-store";

describe("issue create settings store", () => {
  beforeEach(() => {
    useIssueCreateSettingsStore.setState({
      quickCreateFields: DEFAULT_QUICK_CREATE_FIELDS,
      manualCreateFields: DEFAULT_MANUAL_CREATE_FIELDS,
    });
  });

  it("defaults to project-only quick create and the classic manual toolbar", () => {
    expect(useIssueCreateSettingsStore.getState().quickCreateFields).toEqual(["project"]);
    expect(useIssueCreateSettingsStore.getState().manualCreateFields).toEqual([
      "status",
      "priority",
      "executor",
      "labels",
      "project",
    ]);
  });

  it("keeps quick create fields in canonical order regardless of toggle sequence", () => {
    const { setQuickCreateFieldVisible } = useIssueCreateSettingsStore.getState();

    setQuickCreateFieldVisible("due_date", true);
    setQuickCreateFieldVisible("priority", true);

    expect(useIssueCreateSettingsStore.getState().quickCreateFields).toEqual([
      "project",
      "priority",
      "due_date",
    ]);

    setQuickCreateFieldVisible("project", false);
    expect(useIssueCreateSettingsStore.getState().quickCreateFields).toEqual([
      "priority",
      "due_date",
    ]);
  });

  it("toggles manual create fields independently of quick create", () => {
    const { setManualCreateFieldVisible } = useIssueCreateSettingsStore.getState();

    setManualCreateFieldVisible("labels", false);
    setManualCreateFieldVisible("due_date", true);

    expect(useIssueCreateSettingsStore.getState().manualCreateFields).toEqual([
      "status",
      "priority",
      "executor",
      "project",
      "due_date",
    ]);
    expect(useIssueCreateSettingsStore.getState().quickCreateFields).toEqual(["project"]);
  });

  it("is a no-op to re-enable an already visible field", () => {
    const { setManualCreateFieldVisible } = useIssueCreateSettingsStore.getState();

    setManualCreateFieldVisible("status", true);

    expect(useIssueCreateSettingsStore.getState().manualCreateFields).toEqual(
      DEFAULT_MANUAL_CREATE_FIELDS,
    );
  });

  it("migrates legacy assignee visibility to the explicit roles", () => {
    expect(
      migrateIssueCreateSettings(
        {
          quickCreateFields: ["project", "unknown"],
          manualCreateFields: ["status", "assignee", "labels", "unknown"],
        },
        1,
      ),
    ).toEqual({
      quickCreateFields: ["project"],
      manualCreateFields: ["status", "owner", "executor", "labels"],
    });
  });

  it("normalizes unknown fields and preserves an intentional empty selection", () => {
    expect(
      normalizeIssueCreateFields(["unknown"], MANUAL_CREATE_FIELDS, DEFAULT_MANUAL_CREATE_FIELDS),
    ).toEqual(DEFAULT_MANUAL_CREATE_FIELDS);
    expect(
      normalizeIssueCreateFields([], MANUAL_CREATE_FIELDS, DEFAULT_MANUAL_CREATE_FIELDS),
    ).toEqual([]);
  });

  it("resets both modes to product defaults", () => {
    useIssueCreateSettingsStore.setState({
      quickCreateFields: ["project", "priority"],
      manualCreateFields: ["owner", "reviewer"],
    });
    useIssueCreateSettingsStore.getState().resetToDefaults();
    expect(useIssueCreateSettingsStore.getState().quickCreateFields).toEqual(["project"]);
    expect(useIssueCreateSettingsStore.getState().manualCreateFields).toEqual(
      DEFAULT_MANUAL_CREATE_FIELDS,
    );
  });
});
