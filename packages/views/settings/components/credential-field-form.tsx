"use client";

import { type FormEvent, type ReactNode } from "react";
import { Button } from "@orvilo/ui/components/ui/button";
import {
  Field,
  FieldDescription,
  FieldError,
  FieldGroup,
  FieldLabel,
  FieldSeparator,
} from "@orvilo/ui/components/ui/field";
import { Input } from "@orvilo/ui/components/ui/input";
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@orvilo/ui/components/ui/select";

export type CredentialField = {
  id: string;
  label: string;
  value: string;
  onChange: (value: string) => void;
  type?: "text" | "password";
  placeholder?: string;
  description?: string;
  error?: string;
  testId?: string;
  disabled?: boolean;
  autoComplete?: string;
};

export type ConnectionSelectOption = {
  value: string;
  label: string;
};

/**
 * Fill-in layout taken from ReUI `c-field-9`: responsive Field rows, then a
 * separator, optional description, and outline Cancel + default Submit.
 */
export function CredentialFieldForm({
  fields,
  leading,
  description,
  cancelLabel,
  submitLabel,
  submitting = false,
  canSubmit,
  onCancel,
  onSubmit,
  submitTestId,
}: {
  fields: CredentialField[];
  leading?: ReactNode;
  description?: ReactNode;
  cancelLabel: string;
  submitLabel: string;
  submitting?: boolean;
  canSubmit: boolean;
  onCancel: () => void;
  onSubmit: () => void;
  submitTestId?: string;
}) {
  function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!canSubmit) return;
    onSubmit();
  }

  return (
    <form className="w-full" onSubmit={handleSubmit}>
      <FieldGroup>
        {leading}
        {fields.map((field) => (
          <Field
            key={field.id}
            orientation="responsive"
            data-invalid={field.error ? true : undefined}
            data-disabled={field.disabled || submitting ? true : undefined}
          >
            <FieldLabel htmlFor={field.id}>{field.label}</FieldLabel>
            <Input
              id={field.id}
              data-testid={field.testId}
              type={field.type ?? "text"}
              value={field.value}
              onChange={(event) => field.onChange(event.target.value)}
              placeholder={field.placeholder}
              autoComplete={field.autoComplete ?? "off"}
              spellCheck={false}
              disabled={field.disabled || submitting}
              aria-invalid={field.error ? true : undefined}
            />
            {field.description ? (
              <FieldDescription>{field.description}</FieldDescription>
            ) : null}
            {field.error ? <FieldError>{field.error}</FieldError> : null}
          </Field>
        ))}
        <FieldSeparator />
        {description ? <FieldDescription>{description}</FieldDescription> : null}
        <div className="flex justify-end gap-2">
          <Button type="button" variant="outline" onClick={onCancel} disabled={submitting}>
            {cancelLabel}
          </Button>
          <Button type="submit" disabled={!canSubmit} data-testid={submitTestId}>
            {submitLabel}
          </Button>
        </div>
      </FieldGroup>
    </form>
  );
}

export function ConnectionSelectField({
  id,
  label,
  value,
  onValueChange,
  items,
  placeholder,
  disabled,
}: {
  id: string;
  label: string;
  value: string;
  onValueChange: (value: string) => void;
  items: ConnectionSelectOption[];
  placeholder?: string;
  disabled?: boolean;
}) {
  const selectItems = placeholder
    ? [{ value: "", label: placeholder }, ...items]
    : items;

  return (
    <Field orientation="responsive" data-disabled={disabled ? true : undefined}>
      <FieldLabel htmlFor={id}>{label}</FieldLabel>
      <Select
        items={selectItems}
        value={value || null}
        onValueChange={(next) => onValueChange(next ?? "")}
        disabled={disabled}
      >
        <SelectTrigger id={id} className="w-full">
          <SelectValue placeholder={placeholder} />
        </SelectTrigger>
        <SelectContent className="w-(--anchor-width)">
          <SelectGroup>
            {selectItems.map((item) => (
              <SelectItem key={item.value || "empty"} value={item.value}>
                {item.label}
              </SelectItem>
            ))}
          </SelectGroup>
        </SelectContent>
      </Select>
    </Field>
  );
}
