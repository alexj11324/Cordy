"use client";

import { type ComponentProps, type ReactNode } from "react";
import { Badge } from "@orvilo/ui/components/reui/badge";
import { cn } from "@orvilo/ui/lib/utils";

export interface SettingFieldProps {
  title: string;
  description: string;
  badge?: {
    label: string;
    variant: ComponentProps<typeof Badge>["variant"];
  };
  children: ReactNode;
  last?: boolean;
  labelFor?: string;
  contentClassName?: string;
}

export function SettingField({
  title,
  description,
  badge,
  children,
  last,
  labelFor,
  contentClassName,
}: SettingFieldProps) {
  return (
    <>
      <div className="flex flex-col gap-4 px-5 py-4.5 sm:flex-row sm:items-center sm:justify-between">
        <div className="flex min-w-0 flex-1 flex-col gap-1 max-w-md">
          <div className="flex flex-wrap items-center gap-2">
            {labelFor ? (
              <label
                htmlFor={labelFor}
                className="text-body font-medium text-foreground cursor-pointer"
              >
                {title}
              </label>
            ) : (
              <span className="text-body font-medium text-foreground">{title}</span>
            )}

            {badge ? (
              <Badge variant={badge.variant} size="sm">
                {badge.label}
              </Badge>
            ) : null}
          </div>

          <p className="text-caption text-muted-foreground leading-relaxed">
            {description}
          </p>
        </div>

        <div className={cn("min-w-0 w-full sm:w-80 shrink-0", contentClassName)}>
          {children}
        </div>
      </div>

      {!last ? <div className="h-px w-full bg-border/60" /> : null}
    </>
  );
}
