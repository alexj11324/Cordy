/**
 * Assert a LobeHub `<Button>`'s **treatment**, not merely its presence.
 *
 * Why the obvious route does not work. `@lobehub/ui` has no `variant` prop;
 * `type="primary"` reaches the DOM only as an antd-style class, and the
 * library's *default* is the outlined treatment — the opposite of the shadcn
 * `<Button>` this family was migrated from, whose no-`variant` default was
 * `bg-primary` (`packages/ui/components/ui/button.tsx`). A call site can
 * therefore lose its solid primary treatment while every prop stays identical:
 *
 *   <Button onClick title data-testid>  →  <LobeButton onClick title data-testid>
 *
 * so no prop diff can see it. The library *does* export its style object
 * (`@lobehub/ui/es/base-ui/Button/style` → `styles`), but **those values are not
 * the classes that reach the DOM**. Measured in this package: `styles.base` is
 * `acss-10wok4q` and `styles.variantPrimary` is `acss-2gwsvx`, while a rendered
 * `<Button type="primary">` carries the single merged class `acss-1c34g2g` (and
 * no `type` yields `acss-p7gwe`) — none of the exported tokens appear on any
 * element. The bridge's `<StyleProvider hashPriority="low">`
 * (`lobe/lobe-theme-bridge.tsx`) and antd-style's static-style merging both move
 * the hash. Asserting `styles.variantPrimary` there fails *closed*, but it fails
 * for every call site, which is no guard at all.
 *
 * What is stable is the class Lobe computes for a given treatment **here**. So
 * the reference is a render, not a literal: two controls the library paints
 * itself, compared against the element under test. That stays hash-agnostic —
 * when the hash generator moves, both sides move together — and
 * `primary !== outlined` proves in the same run that the library still tells the
 * two apart, so the comparison cannot pass vacuously.
 *
 * The boundary this helper does not cross: it asserts a *library* fact (which
 * class Lobe paints for `type="primary"`). Whether a given control **should** be
 * primary is a product rule, and that assertion stays in the test that makes it.
 */
import { render, within } from "@testing-library/react";
import { expect } from "vitest";

import { Button } from "@lobehub/ui/base-ui";

import { LobeThemeBridge } from "../lobe";

const PRIMARY_REFERENCE_TESTID = "lobe-treatment-reference-primary";
const OUTLINED_REFERENCE_TESTID = "lobe-treatment-reference-outlined";

export interface LobeButtonTreatmentClasses {
  /** The class Lobe paints for `type="primary"` — the solid primary treatment. */
  primary: string;
  /** The class Lobe paints for a `<Button>` with no `type` — the outlined default. */
  outlined: string;
}

let cached: Promise<LobeButtonTreatmentClasses> | null = null;

/**
 * Render the two reference controls under the theme bridge and read back the
 * classes Lobe gave them.
 *
 * Cached for the file: the hash follows the provider config, not the tree, and
 * the classes were measured identical across separate roots, so one render
 * serves every caller (and `afterEach(cleanup)` would tear extra roots down
 * anyway).
 */
export function lobeButtonTreatmentClasses(): Promise<LobeButtonTreatmentClasses> {
  cached ??= (async () => {
    // A detached container: the controls stay out of `document.body`, so the
    // suite's own `screen` queries cannot see them. Measured — the body gains
    // no child.
    const container = document.createElement("div");
    const { unmount } = render(
      <LobeThemeBridge>
        <Button type="primary" data-testid={PRIMARY_REFERENCE_TESTID}>
          Primary
        </Button>
        <Button data-testid={OUTLINED_REFERENCE_TESTID}>Outlined</Button>
      </LobeThemeBridge>,
      { container },
    );
    try {
      const [primary, outlined] = await Promise.all([
        within(container).findByTestId(PRIMARY_REFERENCE_TESTID),
        within(container).findByTestId(OUTLINED_REFERENCE_TESTID),
      ]);
      return { primary: primary.className, outlined: outlined.className };
    } finally {
      // Unmounted rather than left standing: a live bridge takes part in
      // `LobeModalHost`'s owner election, and a stray root holding that slot
      // would portal a later `confirmModal` into this detached container, where
      // the calling test's `screen` queries could not find it. Measured safe —
      // unmounting removes none of the calling tree's styles.
      unmount();
    }
  })();
  return cached;
}

/**
 * Assert `element` carries Lobe's solid primary treatment.
 *
 * The `primary !== outlined` check is what keeps this from being a guard that
 * cannot fail: if a future `@lobehub/ui` painted every button alike, the
 * comparison below would still pass while meaning nothing, and that check is
 * the one that would go red.
 */
export async function expectLobePrimaryTreatment(element: HTMLElement): Promise<void> {
  const { primary, outlined } = await lobeButtonTreatmentClasses();
  expect(primary).not.toBe(outlined);
  expect(element).toHaveClass(primary);
}
