import { useState, useEffect } from "react";
import { cn } from "../../lib/utils";

interface PatchbayIconProps extends React.ComponentProps<"span"> {
  /**
   * If true, play a one-time entrance spin animation.
   */
  animate?: boolean;
  /**
   * If true, disable hover spin animation.
   */
  noSpin?: boolean;
  /**
   * If true, show a border around the icon.
   */
  bordered?: boolean;
  /**
   * Size of the bordered icon: "sm" (default), "md", "lg"
   */
  size?: "sm" | "md" | "lg";
}

const borderedSizes = {
  sm: { wrapper: "p-1.5", icon: "size-3.5" },
  md: { wrapper: "p-2", icon: "size-4" },
  lg: { wrapper: "p-2.5", icon: "size-5" },
};

function PatchbayMark() {
  return (
    <svg
      viewBox="0 0 128 128"
      className="block size-full"
      fill="none"
      aria-hidden="true"
    >
      <g stroke="currentColor" strokeWidth="8">
        <circle cx="64" cy="28" r="10" />
        <circle cx="100" cy="28" r="10" />
        <circle cx="28" cy="64" r="10" />
        <circle cx="100" cy="64" r="10" />
        <circle cx="28" cy="100" r="10" />
        <circle cx="64" cy="100" r="10" />
      </g>
      <g
        stroke="#B6F000"
        strokeWidth="8"
        strokeLinecap="round"
        strokeLinejoin="round"
      >
        <circle cx="28" cy="28" r="10" />
        <path d="M28 38C28 46 34 50 42 50H50C60 50 64 54 64 64V72C64 82 68 86 78 86H82C92 86 96 90 96 100" />
        <circle cx="100" cy="100" r="10" />
      </g>
    </svg>
  );
}

/** Inline rendering of the approved Patchbay routing mark. */
export function PatchbayIcon({
  className,
  animate = false,
  noSpin = false,
  bordered = false,
  size = "sm",
  ...props
}: PatchbayIconProps) {
  const [entranceDone, setEntranceDone] = useState(!animate);

  useEffect(() => {
    if (!animate) return;
    const timer = setTimeout(() => setEntranceDone(true), 600);
    return () => clearTimeout(timer);
  }, [animate]);

  if (bordered) {
    const sizeConfig = borderedSizes[size];
    return (
      <span
        className={cn(
          "inline-flex items-center justify-center border border-border rounded-md",
          sizeConfig.wrapper,
          className
        )}
        aria-hidden="true"
        {...props}
      >
        <span
          className={cn(
            "block",
            sizeConfig.icon,
            !entranceDone && "animate-entrance-spin",
            entranceDone && !noSpin && "hover:animate-spin"
          )}
        >
          <PatchbayMark />
        </span>
      </span>
    );
  }

  return (
    <span
      className={cn(
        "inline-block size-[1em]",
        !entranceDone && "animate-entrance-spin",
        entranceDone && !noSpin && "hover:animate-spin",
        className
      )}
      aria-hidden="true"
      {...props}
    >
      <PatchbayMark />
    </span>
  );
}
