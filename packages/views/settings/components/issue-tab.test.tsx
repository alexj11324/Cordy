import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { cleanup, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import {
  DEFAULT_MANUAL_CREATE_FIELDS,
  DEFAULT_QUICK_CREATE_FIELDS,
  useIssueCreateSettingsStore,
} from "@orvilo/core/issues/stores/issue-create-settings-store";
import { renderWithI18n } from "../../test/i18n";
import { IssueTab } from "./issue-tab";

function resetStore() {
  useIssueCreateSettingsStore.setState({
    quickCreateFields: DEFAULT_QUICK_CREATE_FIELDS,
    manualCreateFields: DEFAULT_MANUAL_CREATE_FIELDS,
  });
}

/**
 * One create mode's group, by its name.
 *
 * Field names repeat across the two groups — Priority, Project and Due date are
 * in both — so a document-wide `getByRole("switch", { name })` would be
 * ambiguous, and to a screen reader the two switches were literally
 * indistinguishable until `SettingsGroup` grew a `role="group"` wrapper named
 * by its own title. This is the handle the reference tells every tab to use,
 * and it also makes each query ~380× cheaper: a document-wide named role query
 * computes the accessible name of every candidate, which against the antd
 * stylesheet costs 3.4s where the scoped one costs 9ms.
 */
async function group(title: string) {
  return within(await screen.findByRole("group", { name: title }));
}

/** `lobe: true` loads the theme bridge on demand, so the first query is async. */
async function renderTab() {
  renderWithI18n(<IssueTab />, { lobe: true });
  await screen.findByRole("switch", { name: "Status" });
}

describe("IssueTab", () => {
  beforeEach(resetStore);

  afterEach(() => {
    cleanup();
    resetStore();
  });

  it("renders a switch per field with the persisted selection", async () => {
    await renderTab();

    // 3 quick create fields + 9 manual create fields.
    expect(screen.getAllByRole("switch")).toHaveLength(12);

    const quick = await group("Create with agent");
    expect(quick.getByRole("switch", { name: "Project" })).toBeChecked();
    expect(quick.getByRole("switch", { name: "Priority" })).not.toBeChecked();
    expect(quick.getByRole("switch", { name: "Due date" })).not.toBeChecked();

    // Manual create defaults to status, priority, executor, labels, project.
    const manual = await group("Create manually");
    for (const field of ["Status", "Priority", "Executor", "Labels", "Project"]) {
      expect(manual.getByRole("switch", { name: field })).toBeChecked();
    }
    for (const field of ["Owner", "Reviewer", "Due date", "Start date"]) {
      expect(manual.getByRole("switch", { name: field })).not.toBeChecked();
    }
  });

  it("persists enabling a quick create field", async () => {
    const user = userEvent.setup();
    await renderTab();

    await user.click(
      (await group("Create with agent")).getByRole("switch", {
        name: "Priority",
      }),
    );

    expect(useIssueCreateSettingsStore.getState().quickCreateFields).toEqual([
      "project",
      "priority",
    ]);
  });

  it("persists hiding a manual create field without touching quick create", async () => {
    const user = userEvent.setup();
    await renderTab();

    await user.click(
      (await group("Create manually")).getByRole("switch", { name: "Labels" }),
    );

    expect(useIssueCreateSettingsStore.getState().manualCreateFields).toEqual([
      "status",
      "priority",
      "executor",
      "project",
    ]);
    expect(useIssueCreateSettingsStore.getState().quickCreateFields).toEqual(["project"]);
  });
});
