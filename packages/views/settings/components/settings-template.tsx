"use client";

import type { ReactNode } from "react";
import { cn } from "@orvilo/ui/lib/utils";
import {
  Field,
  FieldContent,
  FieldDescription,
  FieldError,
  FieldLabel,
  FieldTitle,
} from "@orvilo/ui/components/ui/field";

/**
 * Label-left / control-right row from the flux-agentops account dialog.
 * Copied as-is aside from @orvilo/ui import paths.
 */
export function SettingRow({
  title,
  description,
  children,
  stacked,
  labelFor,
  contentClassName,
  titleAddon,
  className,
  valign = "center",
  error,
}: {
  title: string;
  description?: ReactNode;
  children: ReactNode;
  stacked?: boolean;
  labelFor?: string;
  contentClassName?: string;
  titleAddon?: ReactNode;
  className?: string;
  valign?: "start" | "center";
  error?: { message?: string };
}) {
  return (
    <Field
      orientation={stacked ? "vertical" : "responsive"}
      className={cn(
        "gap-4! px-6 py-0",
        !stacked &&
          "@md/field-group:grid @md/field-group:grid-cols-[minmax(8.25rem,0.74fr)_minmax(0,1.26fr)]",
        !stacked &&
          (valign === "center"
            ? "@md/field-group:items-center @md/field-group:has-[>[data-slot=field-content]]:items-center!"
            : "@md/field-group:items-start"),
        className,
      )}
    >
      <div
        className={cn(
          "flex w-full min-w-0 flex-col gap-0.5",
          stacked ? "w-full" : null,
        )}
      >
        <div className="flex flex-wrap items-center gap-2">
          {labelFor ? (
            <FieldLabel htmlFor={labelFor}>
              <span>{title}</span>
            </FieldLabel>
          ) : (
            <FieldTitle>
              <span>{title}</span>
            </FieldTitle>
          )}
          {titleAddon}
        </div>

        {description ? (
          <FieldDescription className="text-sm">{description}</FieldDescription>
        ) : null}
      </div>

      <FieldContent
        className={cn(
          "w-full max-w-none min-w-0",
          stacked ? "max-w-none" : null,
          contentClassName,
        )}
      >
        <div
          className={cn(
            "flex w-full min-w-0 justify-start",
            valign === "center" ? "items-center" : "items-start",
          )}
        >
          {children}
        </div>
        <FieldError errors={[error]} />
      </FieldContent>
    </Field>
  );
}

export const SETTINGS_FIELD_GROUP_CLASS = "gap-4! py-5";
