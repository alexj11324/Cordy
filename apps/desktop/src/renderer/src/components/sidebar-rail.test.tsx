import { fireEvent, render } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

vi.mock("@patchbay/ui/hooks/use-mobile", () => ({
  useIsCompact: () => false,
}));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: () => "Toggle sidebar" }),
}));

import {
  Sidebar,
  SidebarProvider,
  SidebarRail,
} from "@patchbay/ui/components/ui/sidebar";

describe("SidebarRail", () => {
  it("lets a keyboard activation toggle after a drag click was suppressed", () => {
    const { container } = render(
      <SidebarProvider defaultOpen={false}>
        <Sidebar collapsible="icon">
          <SidebarRail />
        </Sidebar>
      </SidebarProvider>,
    );

    const rail = container.querySelector(
      "[data-slot='sidebar-rail']",
    ) as HTMLButtonElement;
    const sidebar = container.querySelector("[data-slot='sidebar']");
    expect(sidebar).toHaveAttribute("data-state", "collapsed");

    fireEvent.pointerDown(rail, {
      button: 0,
      clientX: 0,
      isPrimary: true,
      pointerId: 1,
    });
    fireEvent.pointerMove(document, {
      clientX: 10,
      pointerId: 1,
    });
    fireEvent.pointerUp(document, { pointerId: 1 });

    fireEvent.click(rail);
    expect(sidebar).toHaveAttribute("data-state", "collapsed");

    // A real keyboard activation produces a click without a pointer gesture.
    fireEvent.click(rail, { detail: 0 });
    expect(sidebar).toHaveAttribute("data-state", "expanded");
  });
});
