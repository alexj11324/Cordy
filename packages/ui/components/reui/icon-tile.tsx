import { mergeProps } from "@base-ui/react/merge-props";
import { useRender } from "@base-ui/react/use-render";
import { cva, type VariantProps } from "class-variance-authority";
import { cn } from "@orvilo/ui/lib/utils";

const iconTileVariants = cva(
  [
    "relative inline-flex shrink-0 items-center justify-center align-middle",
    "size-[var(--icon-tile-size)] rounded-[var(--icon-tile-radius)]",
    "[&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*=size-])]:size-[var(--icon-tile-icon-size)]",
  ],
  {
    variants: {
      variant: {
        outline: "border border-border bg-background dark:bg-input/32",
        elevated:
          "border border-border/70 bg-muted/60 text-foreground shadow-2xs dark:border-border/50 dark:bg-muted/30",
        soft: [
          "isolate p-[var(--icon-tile-inset)] text-primary bg-primary/10",
          "after:absolute after:-z-10 after:inset-[var(--icon-tile-inset)]",
          "after:rounded-[calc(var(--icon-tile-radius)-var(--icon-tile-inset))]",
          "after:border after:border-primary/20 after:bg-primary/5",
        ],
        solid: "bg-primary text-primary-foreground",
        frame: [
          "isolate border border-border bg-muted/50 p-[var(--icon-tile-inset)]",
          "after:absolute after:-z-10 after:inset-[var(--icon-tile-inset)]",
          "after:rounded-[calc(var(--icon-tile-radius)-var(--icon-tile-inset))]",
          "after:border after:border-border after:bg-card shadow-2xs",
        ],
      },
      size: {
        xs: "[--icon-tile-size:1.5rem] [--icon-tile-icon-size:0.875rem] [--icon-tile-inset:0.125rem]",
        sm: "[--icon-tile-size:2rem] [--icon-tile-icon-size:1rem] [--icon-tile-inset:0.125rem]",
        default:
          "[--icon-tile-size:2.5rem] [--icon-tile-icon-size:1.125rem] [--icon-tile-inset:0.1875rem]",
        lg: "[--icon-tile-size:3rem] [--icon-tile-icon-size:1.375rem] [--icon-tile-inset:0.1875rem]",
        xl: "[--icon-tile-size:3.5rem] [--icon-tile-icon-size:1.75rem] [--icon-tile-inset:0.25rem]",
      },
      radius: {
        default: "rounded-lg [--icon-tile-radius:0.5rem]",
        full: "rounded-full [--icon-tile-radius:9999px]",
      },
    },
    defaultVariants: {
      variant: "outline",
      size: "default",
      radius: "default",
    },
  },
);

export interface IconTileProps extends useRender.ComponentProps<"span"> {
  variant?: VariantProps<typeof iconTileVariants>["variant"];
  size?: VariantProps<typeof iconTileVariants>["size"];
  radius?: VariantProps<typeof iconTileVariants>["radius"];
}

export function IconTile({
  className,
  variant = "outline",
  size = "default",
  radius = "default",
  render,
  ...props
}: IconTileProps) {
  const defaultProps = {
    "data-slot": "icon-tile",
    "data-variant": variant,
    "data-size": size,
    className: cn(iconTileVariants({ variant, size, radius, className })),
  };

  return useRender({
    defaultTagName: "span",
    render,
    props: mergeProps<"span">(defaultProps, props),
  });
}

export { iconTileVariants };
