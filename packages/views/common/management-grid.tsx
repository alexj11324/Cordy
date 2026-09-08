"use client";

import { useCallback, useMemo, useRef, type ReactNode } from "react";
import type { Table, TableFeatures } from "@tanstack/react-table";
import {
  DataGrid,
  DataGridContainer,
} from "@orvilo/ui/components/reui/data-grid/data-grid";
import { DataGridTableVirtual } from "@orvilo/ui/components/reui/data-grid/data-grid-table-virtual";
import { useRowLink } from "../navigation";

/**
 * The shared shell every workspace management list renders inside: agents,
 * skills, runtimes, projects, teams. It owns the parts that are the same on
 * all of them and were, under ListGrid, copied per page — including the two
 * layout constraints each copy had to rediscover.
 *
 * The consumer keeps what genuinely differs: its own TanStack table (columns,
 * selection, sorting) and its cells.
 */
interface ManagementGridProps<
  TFeatures extends TableFeatures,
  TData extends object,
> {
  table: Table<TFeatures, TData>;
  recordCount: number;
  emptyMessage: ReactNode;
  /** Fixed row height, in px. The virtualizer's no-measurement contract. */
  rowHeight: number;
  /**
   * Width below which the wide zone scrolls horizontally instead of crushing
   * its columns. Sum of the enabled column widths; an enabled column must
   * never be silently hidden behind a width tier.
   */
  minWidth: number;
  /** Column count, for the clearance spacer's colSpan. */
  columnCount: number;
  /** Detail route for a row id, or null when the row does not navigate. */
  hrefForRow: (rowId: string) => { href: string; title?: string } | null;
}

// Clearance so the last row can scroll clear of floating UI anchored to the
// pane's bottom edge (the chat FAB covers ~48px, a batch toolbar ~62px). It
// rides a spacer row inside the SAME scroller as the data, so it scrolls with
// the content instead of shrinking the viewport.
const BOTTOM_CLEARANCE = 64;

export function ManagementGrid<
  TFeatures extends TableFeatures,
  TData extends object,
>({
  table,
  recordCount,
  emptyMessage,
  rowHeight,
  minWidth,
  columnCount,
  hrefForRow,
}: ManagementGridProps<TFeatures, TData>) {
  const rowLink = useRowLink();

  // Row navigation is DELEGATED from the container rather than spread onto
  // each <tr>: the grid renders its own rows, and its `onRowClick` hands over
  // row data without the MouseEvent - which would drop middle-click (new
  // background tab) and hover prefetch. Rows carry `data-row-id`, so the
  // event target resolves back to a row. Controls inside a row call
  // stopPropagation, exactly as they did under ListGrid.
  const linkForTarget = useCallback(
    (target: EventTarget | null) => {
      if (!(target instanceof Element)) return null;
      const id = target.closest("[data-row-id]")?.getAttribute("data-row-id");
      if (!id) return null;
      const destination = hrefForRow(id);
      if (!destination) return null;
      return { id, link: rowLink(destination.href, destination.title) };
    },
    [hrefForRow, rowLink],
  );
  // onMouseEnter does not bubble, so hover prefetch rides onMouseOver and is
  // latched per row id - otherwise every cell crossed inside one row refires.
  const hoveredRowIdRef = useRef<string | null>(null);

  const footerContent = useMemo(
    () => (
      <tr aria-hidden="true">
        <td
          colSpan={columnCount}
          className="border-0 p-0"
          style={{ height: BOTTOM_CLEARANCE }}
        />
      </tr>
    ),
    [columnCount],
  );

  return (
    <div
      className="min-h-0 flex-1 @container"
      style={
        {
          "--mg-minw": `${minWidth}px`,
          "--mg-rowh": `${rowHeight}px`,
        } as React.CSSProperties
      }
      onClick={(event) => linkForTarget(event.target)?.link.onClick(event)}
      onAuxClick={(event) => linkForTarget(event.target)?.link.onAuxClick(event)}
      onMouseOver={(event) => {
        const hit = linkForTarget(event.target);
        if (hit?.id === hoveredRowIdRef.current) return;
        hoveredRowIdRef.current = hit?.id ?? null;
        hit?.link.onMouseEnter();
      }}
    >
      <DataGrid
        table={table}
        recordCount={recordCount}
        emptyMessage={emptyMessage}
        tableLayout={{
          width: "fixed",
          headerSticky: true,
          headerBackground: true,
          rowBorder: true,
          // Sticky offsets for whatever the consumer pinned; without it the
          // pinned columns scroll away like any other.
          columnsPinnable: true,
        }}
        tableClassNames={{
          base: "@2xl:min-w-[var(--mg-minw)]",
          // h-9 is the management-list header height; the muted band and its
          // bottom border are the explicit header LAYER - without one the
          // column titles read as part of the first row.
          headerRow: "group/header h-9",
          // Faint separators. Identity rows need their height, and with
          // nothing between them the list reads as loose fragments.
          bodyRow:
            "group/row h-[var(--mg-rowh)] cursor-pointer [&>td]:border-border/60",
        }}
      >
        {/* ONE scroll container owns BOTH axes: DataGridTableVirtual's own
            viewport, since no DataGridScrollArea is composed around it.
            Splitting horizontal and vertical scrolling across two elements
            produced a non-converging layout loop (flickering double
            scrollbars) in the lists this replaces. */}
        <DataGridContainer className="h-full">
          <DataGridTableVirtual
            height="100%"
            estimateSize={rowHeight}
            overscan={10}
            footerContent={footerContent}
          />
        </DataGridContainer>
      </DataGrid>
    </div>
  );
}
