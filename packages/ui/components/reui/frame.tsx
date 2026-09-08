import type { ComponentProps } from "react";
import { cva, type VariantProps } from "class-variance-authority";
import { cn } from "@orvilo/ui/lib/utils";

const frameVariants = cva(
  [
    "relative flex flex-col rounded-[var(--frame-radius,0.75rem)] border border-border/80 bg-card/60 text-card-foreground shadow-2xs backdrop-blur-[2px]",
  ],
  {
    variants: {
      variant: {
        default: "border-border/80 bg-card/70",
        inverse: "border-border/60 bg-muted/40",
        ghost: "border-transparent bg-transparent shadow-none",
      },
      spacing: {
        xs: "p-2 gap-2",
        sm: "p-3 gap-3",
        default: "p-4 gap-3",
        lg: "p-5 gap-4",
      },
      stacked: {
        true: "gap-0 divide-y divide-border/60",
        false: "",
      },
      dense: {
        true: "p-0",
        false: "",
      },
    },
    defaultVariants: {
      variant: "default",
      spacing: "default",
      stacked: false,
      dense: false,
    },
  },
);

export function Frame({
  className,
  variant,
  spacing,
  stacked,
  dense,
  ...props
}: ComponentProps<"div"> & VariantProps<typeof frameVariants>) {
  return (
    <div
      className={cn(frameVariants({ variant, spacing, stacked, dense }), className)}
      data-slot="frame"
      {...props}
    />
  );
}

export function FramePanel({
  className,
  fit,
  ...props
}: ComponentProps<"div"> & { fit?: boolean }) {
  return (
    <div
      className={cn(
        "relative overflow-hidden rounded-[calc(var(--frame-radius,0.75rem)-2px)] border border-border/70 bg-card/90 shadow-2xs",
        !fit && "grow",
        className,
      )}
      data-slot="frame-panel"
      {...props}
    />
  );
}

export function FrameHeader({ className, ...props }: ComponentProps<"header">) {
  return (
    <header
      className={cn("flex flex-col gap-1 px-1 py-1", className)}
      data-slot="frame-panel-header"
      {...props}
    />
  );
}

export function FrameTitle({ className, ...props }: ComponentProps<"div">) {
  return (
    <div
      className={cn("text-body font-semibold tracking-tight text-foreground", className)}
      data-slot="frame-panel-title"
      {...props}
    />
  );
}

export function FrameDescription({
  className,
  ...props
}: ComponentProps<"div">) {
  return (
    <div
      className={cn("text-caption text-muted-foreground", className)}
      data-slot="frame-panel-description"
      {...props}
    />
  );
}

export function FrameFooter({ className, ...props }: ComponentProps<"footer">) {
  return (
    <footer
      className={cn("flex items-center gap-2 px-4 py-3 border-t border-border/60 bg-muted/20", className)}
      data-slot="frame-panel-footer"
      {...props}
    />
  );
}

export { frameVariants };
