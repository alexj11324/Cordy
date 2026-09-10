import { forwardRef } from "react";
import type { LucideProps } from "lucide-react";
import { cn } from "../../lib/utils";
import ShareIcon from "../../reui/icons/default/filled/share";

/**
 * Shared dependency graph glyph.
 *
 * The filled ReUI share mark reads as a small set of connected nodes, which
 * keeps dependency affordances consistent across issue and project surfaces.
 * It is decorative wherever this component is used alongside a text label.
 */
export const DependencyIcon = forwardRef<SVGSVGElement, LucideProps>(
  ({ className, size, absoluteStrokeWidth: _absoluteStrokeWidth, ...props }, ref) => (
    <ShareIcon
      ref={ref}
      {...props}
      aria-hidden={props["aria-hidden"] ?? true}
      width={size ?? 24}
      height={size ?? 24}
      className={cn("shrink-0", className)}
    />
  ),
);

DependencyIcon.displayName = "DependencyIcon";
