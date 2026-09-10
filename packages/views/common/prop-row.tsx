import type { ReactNode } from "react";

/**
 * Two-column property row used in detail-page sidebars: a muted label on the
 * left and a flexible value on the right.
 *
 * Uses **subgrid**, so the parent must declare the column tracks:
 *
 *   <div className="grid grid-cols-[auto_1fr] gap-x-2 gap-y-0.5">
 *     <PropRow label="…">…</PropRow>
 *     <PropRow label="…">…</PropRow>
 *   </div>
 *
 * The `auto` track sizes to the widest label across all rows in the parent
 * grid, so labels always fit and values stay aligned across rows without
 * picking a magic pixel width. Earlier versions used a fixed `w-16` label;
 * that broke whenever a label (e.g. "Concurrency") rendered wider than 64px
 * — the label would overflow into the gap and collide with the value.
 *
 * `interactive` (default `true`) highlights only the label and rendered value.
 * The second parent track is flexible, but its empty remainder is not part of
 * the control and must stay visually quiet.
 *
 * Used by:
 *   - issue detail sidebar (Status / Priority / Assignee / …)
 *   - agent detail inspector (Runtime / Model / Visibility / …)
 */
export function PropRow({
  label,
  children,
  interactive = true,
}: {
  label: ReactNode;
  children: ReactNode;
  interactive?: boolean;
}) {
  return (
    <div className="group/prop-row col-span-2 grid min-h-8 grid-cols-subgrid items-center">
      <span
        className={`-ml-2 -mr-2 flex min-w-0 self-stretch items-center gap-1.5 rounded-l-md px-2 text-caption text-muted-foreground ${
          interactive ? "transition-colors group-hover/prop-row:bg-accent/50" : ""
        }`}
      >
        {label}
      </span>
      <div className="flex min-w-0 self-stretch items-stretch text-caption">
        <div
          className={`inline-flex min-w-0 max-w-full items-center gap-1.5 rounded-r-md px-2 ${
            interactive ? "transition-colors group-hover/prop-row:bg-accent/50" : ""
          }`}
        >
          {children}
        </div>
      </div>
    </div>
  );
}
