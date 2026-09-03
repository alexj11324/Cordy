import type { ReactNode } from "react";

/**
 * Full-viewport centering for Clerk's prebuilt card.
 *
 * CSS Grid `place-items: safe center` is the layout primitive for this:
 * the card sits in the middle of the remaining viewport, and `safe` falls
 * back to start-alignment if the card is taller than the viewport so the
 * top of the form stays reachable. No absolute positioning or measured
 * coordinates.
 */
export function ClerkAuthShell({ children }: { children: ReactNode }) {
  return (
    <div className="grid h-full min-h-dvh w-full overflow-y-auto bg-background p-6 [place-items:safe_center]">
      {children}
    </div>
  );
}
