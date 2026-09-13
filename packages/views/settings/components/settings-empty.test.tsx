// @vitest-environment jsdom

import { describe, expect, it, vi } from "vitest";
import { render } from "@testing-library/react";

/**
 * `Empty` is mocked rather than rendered, and the reason is the point of the
 * feature: `tone="danger"` reaches the DOM only as an antd-style hashed class
 * (`es/Text/styles.mjs` maps `type: "danger"` to `color: var(--ant-color-error)`
 * through `createStaticStyles`), so a rendered assertion could do no better than
 * compare class-name strings — coupling the test to the hash generator while
 * still not asserting anything a user can see. That belongs in the renderer
 * pass.
 *
 * What *can* be pinned mechanically is the contract this wrapper owns: the tone
 * becomes `titleProps` on the component that consumes it, and its absence stays
 * absent — which is what keeps every pre-existing consumer byte-identical.
 */
const emptySpy = vi.hoisted(() => vi.fn());

vi.mock("@lobehub/ui/es/Empty/Empty", () => ({
  default: (props: Record<string, unknown>) => {
    emptySpy(props);
    return <div data-testid="empty" />;
  },
}));

import { SettingsEmptyState } from "./settings-empty";

describe("SettingsEmptyState", () => {
  it("passes no title tone by default, so existing consumers are unchanged", () => {
    render(<SettingsEmptyState title="Nothing here" />);
    expect(emptySpy).toHaveBeenCalledWith(
      expect.objectContaining({ title: "Nothing here", titleProps: undefined }),
    );
  });

  it("forwards tone=danger as Lobe's semantic danger title", () => {
    render(<SettingsEmptyState title="Could not load" description="Retry later" tone="danger" />);
    expect(emptySpy).toHaveBeenCalledWith(
      expect.objectContaining({
        title: "Could not load",
        description: "Retry later",
        titleProps: { type: "danger" },
      }),
    );
  });
});
