"use client";

import { Button } from "@orvilo/ui/components/ui/button";
import {
  ScrollArea,
  ScrollBar,
} from "@orvilo/ui/components/ui/scroll-area";
import { cn } from "@orvilo/ui/lib/utils";
import type { ComponentProps } from "react";
import { useCallback } from "react";

export type SuggestionsProps = ComponentProps<typeof ScrollArea> & {
  /** Render a wrapping row without a scroll viewport. */
  scrollable?: boolean;
};

export const Suggestions = ({
  className,
  children,
  scrollable = true,
  ...props
}: SuggestionsProps) => {
  if (!scrollable) {
    return <div className={cn("flex items-center gap-2", className)}>{children}</div>;
  }
  return (
    <ScrollArea className="w-full overflow-x-auto whitespace-nowrap" {...props}>
      <div className={cn("flex w-max flex-nowrap items-center gap-2", className)}>
        {children}
      </div>
      <ScrollBar className="hidden" orientation="horizontal" />
    </ScrollArea>
  );
};

export type SuggestionProps = Omit<ComponentProps<typeof Button>, "onClick"> & {
  suggestion: string;
  onClick?: (suggestion: string) => void;
};

export const Suggestion = ({
  suggestion,
  onClick,
  className,
  variant = "outline",
  size = "sm",
  children,
  ...props
}: SuggestionProps) => {
  const handleClick = useCallback(() => {
    onClick?.(suggestion);
  }, [onClick, suggestion]);

  return (
    <Button
      className={cn("cursor-pointer rounded-full px-4", className)}
      onClick={handleClick}
      size={size}
      type="button"
      variant={variant}
      {...props}
    >
      {children || suggestion}
    </Button>
  );
};
