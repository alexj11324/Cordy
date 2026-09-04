import type {
  ButtonHTMLAttributes,
  ComponentProps,
  InputHTMLAttributes,
  ReactNode,
  TextareaHTMLAttributes,
} from "react";
import { AlertCircle, Check, Loader2, Search, type LucideIcon } from "lucide-react";
import { Input } from "@patchbay/ui/components/ui/input";
import { SelectTrigger } from "@patchbay/ui/components/ui/select";
import { Textarea } from "@patchbay/ui/components/ui/textarea";
import { cn } from "@patchbay/ui/lib/utils";

export type SettingsSaveStatus = "idle" | "saving" | "saved" | "error";

/**
 * Transparent field chrome for stacked settings rows that edit in place.
 * Cancels the shared Input/Textarea border, height, and focus ring so the
 * control reads as the value line under the label.
 */
export const SETTINGS_INLINE_FIELD_CLASS =
  "h-auto min-h-0 rounded-none border-0 bg-transparent px-0 py-0 text-body text-muted-foreground shadow-none placeholder:text-muted-foreground focus-visible:border-transparent focus-visible:ring-0 focus-visible:ring-transparent dark:bg-transparent";

/**
 * Filled pill chrome for settings controls (selects, search, single-line
 * inputs). Replaces Linear/Multica bordered fields so the inner page matches
 * the Buzz shell rather than looking like a second product.
 */
export const SETTINGS_CONTROL_CLASS =
  "h-8 w-full rounded-full border-transparent bg-muted px-3 text-body shadow-none hover:bg-muted/80 focus-visible:border-transparent focus-visible:ring-2 focus-visible:ring-ring dark:bg-muted dark:hover:bg-muted/80";

export const SETTINGS_TEXTAREA_CLASS =
  "min-h-[4.5rem] rounded-xl border-transparent bg-muted px-3 py-2 text-body shadow-none hover:bg-muted/80 focus-visible:border-transparent focus-visible:ring-2 focus-visible:ring-ring dark:bg-muted dark:hover:bg-muted/80";

export function SettingsTab({
  title,
  description,
  action,
  children,
}: {
  title: ReactNode;
  description?: ReactNode;
  action?: ReactNode;
  children: ReactNode;
}) {
  const copy = (
    <>
      <h2 className="text-display-sm font-semibold tracking-tight">{title}</h2>
      {description ? (
        <p className="text-title-sm font-normal text-muted-foreground">
          {description}
        </p>
      ) : null}
    </>
  );

  return (
    <div>
      {action ? (
        <header className="mb-12 flex min-w-0 items-start justify-between gap-4">
          <div className="min-w-0 space-y-1">{copy}</div>
          <div className="shrink-0">{action}</div>
        </header>
      ) : (
        <header className="mb-12 min-w-0 space-y-1">{copy}</header>
      )}
      <div className="space-y-12">{children}</div>
    </div>
  );
}

export function SettingsSection({
  title,
  description,
  action,
  children,
  className,
}: {
  title?: ReactNode;
  description?: ReactNode;
  action?: ReactNode;
  children: ReactNode;
  className?: string;
}) {
  return (
    <section className={cn("space-y-2", className)}>
      {title || description || action ? (
        <div
          className="flex min-h-7 items-end gap-3 px-4"
          data-slot="settings-section-header"
        >
          <div className="min-w-0 flex-1">
            {title ? (
              <h3 className="text-body font-semibold text-muted-foreground">
                {title}
              </h3>
            ) : null}
            {description ? (
              <p className="mt-0.5 text-body font-normal text-muted-foreground">
                {description}
              </p>
            ) : null}
          </div>
          {action ? <div className="shrink-0">{action}</div> : null}
        </div>
      ) : null}
      {children}
    </section>
  );
}

export function SettingsCard({
  children,
  className,
}: {
  children: ReactNode;
  className?: string;
}) {
  return (
    <div
      data-slot="settings-section-card"
      className={cn(
        "[container-type:inline-size] divide-y divide-border overflow-hidden rounded-xl border border-border bg-surface",
        className,
      )}
    >
      {children}
    </div>
  );
}

/**
 * Width tiers for the control column. Within a card, every text-entry
 * control shares the `text` tier so their edges align; a row may only
 * drop to a smaller tier when the field is deliberately short (a code,
 * an enum select) — the difference must read as intentional. Pick a
 * tier instead of adding per-row ad-hoc widths.
 *
 * Widths bind to the card's container, not the viewport, so a settings
 * card in a narrow inspector stacks and shrinks the same way it does
 * on a phone.
 */
const SETTINGS_CONTROL_WIDTHS = {
  /** Text inputs and textareas — the standard control column. */
  text: "[@container(min-width:34rem)]:w-96",
  /** Selects/pickers with long option labels (timezone, model). */
  "select-wide": "[@container(min-width:34rem)]:w-72",
  /** Compact enum selects (theme, language). */
  select: "[@container(min-width:34rem)]:w-48",
  /** Short fixed-format codes (issue prefix). */
  code: "[@container(min-width:34rem)]:w-40",
  /** Unconstrained — non-input content like avatar uploads. */
  none: "[@container(min-width:34rem)]:max-w-none",
} as const;

export type SettingsControlSize = keyof typeof SETTINGS_CONTROL_WIDTHS;

export function SettingsRow({
  label,
  description,
  children,
  className,
  size,
  align = "center",
  layout = "split",
  htmlFor,
}: {
  label: ReactNode;
  description?: ReactNode;
  children: ReactNode;
  className?: string;
  /** Control column width tier; omit for content-hugging controls (buttons, switches). */
  size?: SettingsControlSize;
  align?: "center" | "start";
  /**
   * `split` is label left / control right (Appearance, Preferences).
   * `stack` is label above the value, used for read/edit profile fields.
   */
  layout?: "split" | "stack";
  htmlFor?: string;
}) {
  const labelNode = htmlFor ? (
    <label htmlFor={htmlFor} className="block text-body font-medium">
      {label}
    </label>
  ) : (
    <div className="text-body font-medium">{label}</div>
  );

  const descriptionNode = description ? (
    <div className="mt-0.5 text-body font-normal text-muted-foreground">
      {description}
    </div>
  ) : null;

  if (layout === "stack") {
    return (
      <div
        className={cn(
          "flex min-h-16 items-center gap-4 px-4 py-3 text-body",
          className,
        )}
      >
        <div className="min-w-0 flex-1 space-y-1">
          {labelNode}
          {descriptionNode}
          {children}
        </div>
      </div>
    );
  }

  return (
    <div
      className={cn(
        "flex min-h-16 flex-col gap-3 px-4 py-3 text-body [@container(min-width:34rem)]:flex-row [@container(min-width:34rem)]:justify-between [@container(min-width:34rem)]:gap-4",
        align === "center"
          ? "[@container(min-width:34rem)]:items-center"
          : "[@container(min-width:34rem)]:items-start",
        className,
      )}
    >
      <div className="min-w-0 flex-1">
        {labelNode}
        {descriptionNode}
      </div>
      <div
        className={cn(
          "w-full shrink-0 [@container(min-width:34rem)]:w-auto [@container(min-width:34rem)]:max-w-[56%]",
          size ? SETTINGS_CONTROL_WIDTHS[size] : undefined,
        )}
      >
        {children}
      </div>
    </div>
  );
}

/** Name + meta on the left, actions on the right — catalog rows inside a card. */
export function SettingsListRow({
  children,
  className,
}: {
  children: ReactNode;
  className?: string;
}) {
  return (
    <div
      className={cn(
        "flex min-h-16 items-center gap-4 px-4 py-3 text-body",
        className,
      )}
    >
      {children}
    </div>
  );
}

export function SettingsEmpty({
  title,
  description,
  className,
}: {
  title: ReactNode;
  description?: ReactNode;
  className?: string;
}) {
  return (
    <div className={cn("px-4 py-10 text-center", className)}>
      <p className="text-body font-medium">{title}</p>
      {description ? (
        <p className="mx-auto mt-1 max-w-md text-body text-muted-foreground">
          {description}
        </p>
      ) : null}
    </div>
  );
}

export function SettingsPillButton({
  children,
  icon: Icon,
  active = false,
  tone,
  className,
  type = "button",
  ...props
}: ButtonHTMLAttributes<HTMLButtonElement> & {
  icon?: LucideIcon;
  active?: boolean;
  tone?: "muted" | "primary" | "destructive";
}) {
  const resolvedTone = tone ?? (active ? "primary" : "muted");
  return (
    <button
      type={type}
      className={cn(
        "inline-flex shrink-0 items-center gap-1.5 rounded-full border px-3 py-1.5 text-body font-medium transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 disabled:cursor-not-allowed disabled:opacity-60",
        resolvedTone === "primary" &&
          "border-transparent bg-primary text-primary-foreground shadow-sm hover:bg-primary/90",
        resolvedTone === "muted" &&
          "border-transparent bg-muted text-foreground hover:bg-muted/80",
        resolvedTone === "destructive" &&
          "border-transparent bg-destructive/10 text-destructive hover:bg-destructive/15",
        className,
      )}
      {...props}
    >
      {Icon ? <Icon className="size-4 shrink-0" /> : null}
      {children}
    </button>
  );
}

export function SettingsIconButton({
  className,
  type = "button",
  children,
  ...props
}: ButtonHTMLAttributes<HTMLButtonElement>) {
  return (
    <button
      type={type}
      className={cn(
        "inline-flex size-8 shrink-0 items-center justify-center rounded-full bg-muted text-muted-foreground transition-colors hover:bg-muted/80 hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-60",
        className,
      )}
      {...props}
    >
      {children}
    </button>
  );
}

export function SettingsField({
  className,
  ...props
}: InputHTMLAttributes<HTMLInputElement>) {
  return <Input className={cn(SETTINGS_CONTROL_CLASS, className)} {...props} />;
}

export function SettingsTextarea({
  className,
  ...props
}: TextareaHTMLAttributes<HTMLTextAreaElement>) {
  return (
    <Textarea className={cn(SETTINGS_TEXTAREA_CLASS, className)} {...props} />
  );
}

export function SettingsSelectTrigger({
  className,
  ...props
}: ComponentProps<typeof SelectTrigger>) {
  return (
    <SelectTrigger
      className={cn(SETTINGS_CONTROL_CLASS, "justify-between", className)}
      {...props}
    />
  );
}

export function SettingsSearchField({
  value,
  onValueChange,
  placeholder,
  className,
  id,
}: {
  value: string;
  onValueChange: (value: string) => void;
  placeholder: string;
  className?: string;
  id?: string;
}) {
  return (
    <div className={cn("relative min-w-0", className)}>
      <Search className="pointer-events-none absolute left-3 top-1/2 size-4 -translate-y-1/2 text-muted-foreground" />
      <Input
        id={id}
        value={value}
        onChange={(event) => onValueChange(event.target.value)}
        placeholder={placeholder}
        aria-label={placeholder}
        className={cn(SETTINGS_CONTROL_CLASS, "pl-9")}
      />
    </div>
  );
}

export function SettingsSaveState({
  status,
  savingLabel,
  savedLabel,
  errorLabel,
}: {
  status: SettingsSaveStatus;
  savingLabel: string;
  savedLabel: string;
  errorLabel: string;
}) {
  if (status === "idle") return null;

  const content =
    status === "saving" ? (
      <>
        <Loader2 className="size-3 animate-spin" />
        {savingLabel}
      </>
    ) : status === "saved" ? (
      <>
        <Check className="size-3 text-success" />
        {savedLabel}
      </>
    ) : (
      <>
        <AlertCircle className="size-3 text-destructive" />
        {errorLabel}
      </>
    );

  return (
    <span
      role="status"
      className={cn(
        "inline-flex items-center gap-1.5 text-caption text-muted-foreground",
        status === "error" && "text-destructive",
      )}
    >
      {content}
    </span>
  );
}
