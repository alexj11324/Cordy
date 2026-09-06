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
      <path fill="currentColor" d="M47 39C48 26 56 20 68 20H85C100 20 108 28 108 43V64C108 79 99 87 86 87H67C79 79 85 70 85 59C85 47 77 39 64 39Z" />
      <path fill="currentColor" d="M81 89C80 102 72 108 60 108H43C28 108 20 100 20 85V64C20 49 29 41 42 41H61C49 49 43 58 43 69C43 81 51 89 64 89Z" />
    </svg>
  );
}

/** Inline rendering of the approved Orvilo mark. */
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
