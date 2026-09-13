// @vitest-environment jsdom

import { afterEach, describe, expect, it } from "vitest";
import "@testing-library/jest-dom/vitest";
import { cleanup, screen } from "@testing-library/react";
import { renderWithI18n } from "../../test/i18n";
import { IntegrationSetupGuide } from "./integration-setup-guide";

afterEach(cleanup);

const channels = ["lark", "slack", "dingtalk", "wecom", "telegram", "weixin"] as const;

// `{ lobe: true }` is required, not decoration: this suite used a bare `render`
// with its own `I18nProvider`, so it never reached `LobeThemeBridge` while the
// guide was on shadcn buttons. The two buttons are Lobe's now, and Lobe's
// `Button` throws `Please wrap your app with <ConfigProvider> (or
// <MotionProvider>)` from `useMotionComponent` without the bridge. A test file
// is a host surface, and it is the one that appears in no screenshot.
describe("IntegrationSetupGuide", () => {
  it.each(channels)("keeps the complete %s setup checklist on the integrations page", async (channel) => {
    renderWithI18n(<IntegrationSetupGuide channel={channel} />, { lobe: true });

    const guide = await screen.findByTestId(`integration-setup-guide-${channel}`);
    expect(guide).toHaveTextContent("What you need");
    expect(guide).toHaveTextContent("Complete these steps");
    // The assertion used to be a structural count — `querySelectorAll("ol > li")`
    // had to be 3 — which pinned the *shape* of the markup rather than the
    // procedure it exists for. The list is still an `<ol>` (Lobe has no ordered
    // list, and a stack of flex rows would drop the `list-decimal` semantics a
    // three-step procedure is exactly what `<ol>` is for), so the count did not
    // go red on conversion; it is rewritten anyway because a bare length says
    // nothing about which steps survived. What the three assertions below pin
    // is exactly that and no more: there are three steps, none is blank, and no
    // two are the same.
    const steps = [...guide.querySelectorAll("ol > li")].map((li) => li.textContent);
    expect(steps).toHaveLength(3);
    for (const step of steps) expect(step?.trim()).not.toBe("");
    expect(new Set(steps).size).toBe(3);
  });

  it("links the Slack manifest instructions before the app dashboard", async () => {
    renderWithI18n(<IntegrationSetupGuide channel="slack" />, { lobe: true });

    expect(await screen.findByRole("button", { name: "View Orvilo manifest" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Open Slack app dashboard" })).toBeInTheDocument();
  });

  it("keeps hosted Slack setup inside the managed OAuth flow", async () => {
    renderWithI18n(<IntegrationSetupGuide channel="slack" managed />, { lobe: true });

    expect(await screen.findByText(/Click Connect Slack on this page/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "View Orvilo manifest" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Open Slack app dashboard" })).not.toBeInTheDocument();
  });
});
