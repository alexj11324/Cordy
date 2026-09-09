"use client";

import { ScrollArea as ScrollAreaPrimitive } from "@base-ui/react/scroll-area";
import { forwardRef, type ComponentPropsWithoutRef } from "react";
import { cn } from "@orvilo/ui/lib/utils";

/** Horizontal board scroller adapted from ReUI Pro kanban-board-2. */
export const BoardScrollArea = forwardRef<
  HTMLDivElement,
  ComponentPropsWithoutRef<typeof ScrollAreaPrimitive.Viewport>
>(function BoardScrollArea({ children, className, ...viewportProps }, ref) {
  return (
    <ScrollAreaPrimitive.Root
      data-slot="scroll-area"
      className="relative flex min-h-0 w-full min-w-0 flex-1 overflow-hidden pb-3"
    >
      <ScrollAreaPrimitive.Viewport
        ref={ref}
        data-slot="scroll-area-viewport"
        className={cn(
          "h-full min-h-0 w-full min-w-0 rounded-lg outline-none transition-[color,box-shadow] focus-visible:ring-[3px] focus-visible:ring-ring/50 focus-visible:outline-1",
          className,
        )}
        {...viewportProps}
      >
        <ScrollAreaPrimitive.Content
          data-slot="scroll-area-content"
          className="flex h-full min-w-full"
        >
          {children}
        </ScrollAreaPrimitive.Content>
      </ScrollAreaPrimitive.Viewport>
      <ScrollAreaPrimitive.Scrollbar
        data-slot="scroll-area-scrollbar"
        data-orientation="horizontal"
        orientation="horizontal"
        className="flex touch-none select-none p-px transition-colors data-horizontal:h-2.5 data-horizontal:flex-col data-horizontal:border-t data-horizontal:border-t-transparent data-vertical:h-full data-vertical:w-2.5 data-vertical:border-l data-vertical:border-l-transparent"
      >
        <ScrollAreaPrimitive.Thumb
          data-slot="scroll-area-thumb"
          className="relative flex-1 rounded-full bg-foreground/15"
        />
      </ScrollAreaPrimitive.Scrollbar>
      <ScrollAreaPrimitive.Corner />
    </ScrollAreaPrimitive.Root>
  );
});
