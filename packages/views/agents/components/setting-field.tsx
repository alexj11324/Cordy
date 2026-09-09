"use client";

import { type ComponentProps, type ReactNode } from "react";
import { Badge } from "@orvilo/ui/components/reui/badge";
import { cn } from "@orvilo/ui/lib/utils";
import {
  Field,
  FieldContent,
  FieldDescription,
  FieldLabel,
  FieldSeparator,
  FieldTitle,
} from "@orvilo/ui/components/ui/field";

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

/**
 * One labelled row of a settings panel: title, description and its control.
 * Vendored from the ReUI `settings-3` block; the only local change is the
 * description font size, which uses this repo's role-named `--text-*` scale.
 */
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
      <Field orientation="responsive" className="gap-4 px-4 py-4">
        <div className="flex min-w-0 flex-1 flex-col gap-0.5 @md/field-group:max-w-sm">
          <div className="flex flex-wrap items-center gap-2">
            {labelFor ? (
              <FieldLabel htmlFor={labelFor}>{title}</FieldLabel>
            ) : (
              <FieldTitle>{title}</FieldTitle>
            )}

            {badge ? (
              <Badge variant={badge.variant} size="sm">
                {badge.label}
              </Badge>
            ) : null}
          </div>

          <FieldDescription className="text-caption">
            {description}
          </FieldDescription>
        </div>

        <FieldContent
          className={cn("min-w-0 @md/field-group:w-78", contentClassName)}
        >
          {children}
        </FieldContent>
      </Field>

      {!last ? <FieldSeparator /> : null}
    </>
  );
}
