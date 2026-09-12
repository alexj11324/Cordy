import Form from "@lobehub/ui/es/Form/index";
import { screen } from "@testing-library/react";
import type { ReactElement } from "react";
import { describe, expect, it } from "vitest";

import { renderWithI18n } from "../../test/i18n";
import { SettingsFormRow, SettingsGroup } from "./settings-shell";

// `lobe: true` mounts the theme bridge; the first query therefore has to be an
// async one (`findBy`), because the bridge is loaded on demand.
//
// The shell deep-imports `Form` (`@lobehub/ui/es/Form/index`) where the rest of
// the app reaches it through the package root, so the two have to resolve to one
// module — two instances would mean two antd `Form` contexts and a `Form.Item`
// that cannot see the `Form` around it. That was verified by execution rather
// than kept as an assertion here: importing the root into this file to compare
// them costs 18s (3.07s → 21.30s, `tests` alone 1.41s → 11.12s), which is the
// fan-out cost this module exists to avoid, re-created inside a test. What is
// kept, in `lobe/lobe-theme-bridge.test.tsx`, is the weaker but still
// load-bearing half: `expect(DeepModalHost).toBe(BarrelModalHost)` proves this
// package resolves to a single instance across a barrel and a deep path. It
// compares a different module, so it does not by itself prove the two `Form`
// specifiers agree — only the resolution guarantee behind them.

describe("SettingsGroup", () => {
  it("renders the section header, its action and its rows", async () => {
    renderWithI18n(
      <SettingsGroup
        title="Appearance"
        description="How Orvilo looks"
        extra={<button type="button">Reset</button>}
      >
        <div>Row body</div>
      </SettingsGroup>,
      { lobe: true },
    );

    expect(await screen.findByText("Appearance")).toBeTruthy();
    expect(screen.getByText("How Orvilo looks")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Reset" })).toBeTruthy();
    expect(screen.getByText("Row body")).toBeTruthy();
  });

  // `Form.Group` derives `collapsible` from the variant when it is undefined,
  // and an outlined group is collapsible by default. That turns the section
  // title into a disclosure toggle, so a stray click hides every row beneath
  // it. The title must not be a button.
  //
  // An earlier revision of this comment added a second reason — "breaks the
  // settings nav search, because it matches on row content, and a collapsed
  // section matches nothing". That reason was not real: the page's only search
  // is `matches()` in `settings-page.tsx`, which filters *rail tabs* on their
  // value, label and description, and nothing on the page looks at row
  // content. The collapsible default is reason enough on its own.
  // The group's title is the only thing that tells two same-named rows apart,
  // and rc-collapse sets no role at all unless `accordion` is true, so the
  // wrapper is what makes the section addressable: `role="group"` named by
  // `aria-labelledby` pointing at the title element itself. Every tab's tests
  // scope their queries with this, so it is pinned here.
  it("is an addressable group named by its title", async () => {
    renderWithI18n(
      <SettingsGroup title="Appearance">
        <div>Row body</div>
      </SettingsGroup>,
      { lobe: true },
    );

    const group = await screen.findByRole("group", { name: "Appearance" });
    expect(group).toBeTruthy();
    // The name comes from the visible title, not a copy of it.
    const labelId = group.getAttribute("aria-labelledby");
    expect(labelId).toBeTruthy();
    expect(document.getElementById(labelId as string)?.textContent).toBe(
      "Appearance",
    );
  });

  // A group with no title has no name to be addressed by, and an unnamed group
  // role adds nothing — the shortcuts tab's toolbar is exactly that case.
  it("adds no group role when it has no title", async () => {
    renderWithI18n(
      <SettingsGroup extra={<button type="button">Reset</button>}>
        <div>Row body</div>
      </SettingsGroup>,
      { lobe: true },
    );

    expect(await screen.findByText("Row body")).toBeTruthy();
    expect(screen.queryAllByRole("group")).toHaveLength(0);
  });

  // An empty string is the shape a conditional title arrives in —
  // `title={showHeader ? label : ""}` — and it is not a title. `role="group"`
  // with an empty `aria-labelledby` announces a group with no name, which is
  // worse than no group at all, so it must take the untitled branch.
  it("treats an empty title as no title", async () => {
    renderWithI18n(
      <SettingsGroup title="">
        <div>Row body</div>
      </SettingsGroup>,
      { lobe: true },
    );

    expect(await screen.findByText("Row body")).toBeTruthy();
    expect(screen.queryAllByRole("group")).toHaveLength(0);
  });

  it("does not make the section title a collapse toggle", async () => {
    renderWithI18n(
      <SettingsGroup title="Appearance">
        <div>Row body</div>
      </SettingsGroup>,
      { lobe: true },
    );

    expect(await screen.findByText("Appearance")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Appearance" })).toBeNull();
  });

  // `variant` decides the card's chrome, and it reaches us through a prop this
  // wrapper forwards. The oracle is a raw `Form.Group` rendered by the library
  // with the same props, because the variant only shows up in antd-style's
  // hashed class names — a comparison between our own two variants would pass
  // just as well with the default flipped to `borderless`.
  it("defaults to the outlined variant and forwards borderless", async () => {
    // Compares the collapse root's class list, not its subtree: `SettingsGroup`
    // wraps the title in a `<span>` to carry the group's `aria-labelledby`, so
    // our tree differs from a raw `Form.Group`'s in a way this test is not
    // about. What it *is* about — that `variant` reaches the library unchanged
    // — is a class on that root.
    async function groupHtml(node: ReactElement): Promise<string> {
      const { container, unmount } = renderWithI18n(node, { lobe: true });
      // Waits out the on-demand bridge load before reading the tree.
      await screen.findByText("Appearance");
      const html =
        container.querySelector('[class*="ant-collapse"]')?.className ?? "";
      unmount();
      // antd mints a fresh `css-var-_r_N_` scope class per mount, so two
      // structurally identical trees never compare equal verbatim.
      return html.replaceAll(/css-var-[\w-]+/g, "css-var-*");
    }

    const children = <div>Row body</div>;
    const outlined = await groupHtml(
      <SettingsGroup title="Appearance">{children}</SettingsGroup>,
    );
    const borderless = await groupHtml(
      <SettingsGroup title="Appearance" variant="borderless">
        {children}
      </SettingsGroup>,
    );
    const rawOutlined = await groupHtml(
      <Form.Group collapsible={false} title="Appearance" variant="outlined">
        {children}
      </Form.Group>,
    );

    expect(outlined).not.toBe("");
    expect(outlined).not.toBe(borderless);
    expect(outlined).toBe(rawOutlined);
  });
});

describe("SettingsFormRow", () => {
  it("renders its label, description and control", async () => {
    renderWithI18n(
      <SettingsFormRow label="Theme" description="Applies to this device">
        <span>control</span>
      </SettingsFormRow>,
      { lobe: true },
    );

    expect(await screen.findByText("Theme")).toBeTruthy();
    expect(screen.getByText("Applies to this device")).toBeTruthy();
    expect(screen.getByText("control")).toBeTruthy();
  });

  it("reserves the control column from minWidth", async () => {
    const { container } = renderWithI18n(
      <SettingsFormRow label="Theme" minWidth={384}>
        <span>control</span>
      </SettingsFormRow>,
      { lobe: true },
    );

    await screen.findByText("Theme");

    const row = container.querySelector<HTMLElement>(
      "[style*='--form-item-min-width']",
    );
    expect(row?.style.getPropertyValue("--form-item-min-width")).toBe("384px");
  });

  // antd turns the label colon ON by default and only suppresses it when it is
  // told to: `computedColon = colon === true || (contextColon !== false && colon
  // !== false)`, and with no enclosing `Form` every term of that is `undefined`.
  // So omitting `colon` is what renders it, and `SettingsFormRow` passes
  // `colon={false}`. This is the guard for that: the failure is silent, it
  // reappears the moment anyone rebuilds the row, and the colon is generated
  // content rather than text, so it can only be caught here by the state antd
  // reports — `ant-form-item-no-colon` on the label — plus the label text.
  it("suppresses antd's label colon", async () => {
    const { container } = renderWithI18n(
      <SettingsFormRow label="Theme" description="Applies to this device">
        <span>control</span>
      </SettingsFormRow>,
      { lobe: true },
    );

    const label = (await screen.findByText("Theme")).closest("label");
    expect(label?.className).toContain("ant-form-item-no-colon");
    expect(label?.textContent ?? "").not.toMatch(/[:：]\s*$/);
    expect(container.querySelector(".ant-form-item")).toBeTruthy();
  });

  it("draws a separator above a row only when asked", async () => {
    renderWithI18n(
      <>
        <SettingsFormRow label="First">
          <span>first control</span>
        </SettingsFormRow>
        <SettingsFormRow label="Second" divider>
          <span>second control</span>
        </SettingsFormRow>
      </>,
      { lobe: true },
    );

    await screen.findByText("First");
    expect(screen.getAllByRole("separator")).toHaveLength(1);
  });
});
