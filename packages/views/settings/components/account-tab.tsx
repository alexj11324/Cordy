/* eslint-disable i18next/no-literal-string -- profile-3 is a registry block with intentional English copy; localize this data layer without changing its layout. */
/* eslint-disable no-restricted-syntax -- the copied profile-3 labels and technical control text remain literal until the settings copy is localized. */

"use client";

import {
  useCallback,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react";
import { Badge } from "@orvilo/ui/components/reui/badge";
import { PhoneInput } from "@orvilo/ui/components/reui/phone-input";
import { Button } from "@orvilo/ui/components/ui/button";
import { Card, CardContent, CardFooter } from "@orvilo/ui/components/ui/card";
import {
  Combobox,
  ComboboxCollection,
  ComboboxContent,
  ComboboxEmpty,
  ComboboxGroup,
  ComboboxInput,
  ComboboxItem,
  ComboboxLabel,
  ComboboxList,
} from "@orvilo/ui/components/ui/combobox";
import {
  Field,
  FieldDescription,
  FieldGroup,
  FieldLabel,
} from "@orvilo/ui/components/ui/field";
import { Input } from "@orvilo/ui/components/ui/input";
import { Textarea } from "@orvilo/ui/components/ui/textarea";
import {
  InputOTP,
  InputOTPGroup,
  InputOTPSlot,
} from "@orvilo/ui/components/ui/input-otp";
import {
  InputGroup,
  InputGroupAddon,
  InputGroupButton,
  InputGroupInput,
  InputGroupText,
} from "@orvilo/ui/components/ui/input-group";
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@orvilo/ui/components/ui/select";
import { Separator } from "@orvilo/ui/components/ui/separator";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@orvilo/ui/components/ui/tooltip";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@orvilo/ui/components/ui/alert-dialog";
import { InfoIcon } from "lucide-react";
import { toast } from "sonner";
import { useAuthStore } from "@orvilo/core/auth";
import { api } from "@orvilo/core/api";
import type { SupportedLocale } from "@orvilo/core/i18n";
import { useLocaleAdapter } from "@orvilo/core/i18n/react";
import type { User, UserProfileDetails } from "@orvilo/core/types";
import { AvatarUploadControl } from "../../common/avatar-upload-control";
import { useT } from "../../i18n";
import { SettingsSaveState } from "./settings-layout";
import { useAutoSave } from "./use-auto-save";

interface ProfileFormState {
  firstName: string;
  lastName: string;
  preferredName: string;
  username: string;
  role: string;
  profileDescription: string;
  phone: string;
  website: string;
  timezone: string;
  startWeek: string;
  language: string;
  timeFormat: string;
}

interface SelectOption {
  value: string;
  label: string;
  description?: string;
}

interface TimezoneGroup {
  value: string;
  items: Array<{ value: string; label: string }>;
}

const ROLE_OPTIONS: SelectOption[] = [
  {
    value: "product-ops",
    label: "Product Operations",
    description: "Owns launch, enablement, and process design.",
  },
  {
    value: "growth-lead",
    label: "Growth Lead",
    description: "Shapes lifecycle messaging and experiment rollout.",
  },
  {
    value: "customer-ops",
    label: "Customer Operations",
    description: "Handles escalations, handoff quality, and support workflows.",
  },
];

const LANGUAGE_OPTIONS: SelectOption[] = [
  { value: "en", label: "English" },
  { value: "zh-Hans", label: "简体中文" },
];

const START_WEEK_OPTIONS: SelectOption[] = [
  { value: "monday", label: "Monday" },
  { value: "sunday", label: "Sunday" },
  { value: "saturday", label: "Saturday" },
];

const TIME_FORMAT_OPTIONS: SelectOption[] = [
  { value: "24-hour", label: "24-hour" },
  { value: "12-hour", label: "12-hour" },
];

const TIMEZONE_GROUPS: TimezoneGroup[] = [
  {
    value: "Americas",
    items: [
      { value: "America/Los_Angeles", label: "(GMT-8) Los Angeles" },
      { value: "America/Denver", label: "(GMT-7) Denver" },
      { value: "America/Chicago", label: "(GMT-6) Chicago" },
      { value: "America/New_York", label: "(GMT-5) New York" },
      { value: "America/Toronto", label: "(GMT-5) Toronto" },
    ],
  },
  {
    value: "Europe",
    items: [
      { value: "Europe/London", label: "(GMT+0) London" },
      { value: "Europe/Berlin", label: "(GMT+1) Berlin" },
      { value: "Europe/Paris", label: "(GMT+1) Paris" },
      { value: "Europe/Amsterdam", label: "(GMT+1) Amsterdam" },
      { value: "Europe/Athens", label: "(GMT+2) Athens" },
    ],
  },
  {
    value: "Asia Pacific",
    items: [
      { value: "Asia/Dubai", label: "(GMT+4) Dubai" },
      { value: "Asia/Tashkent", label: "(GMT+5) Tashkent" },
      { value: "Asia/Singapore", label: "(GMT+8) Singapore" },
      { value: "Asia/Tokyo", label: "(GMT+9) Tokyo" },
      { value: "Australia/Sydney", label: "(GMT+11) Sydney" },
    ],
  },
];

function profileDetailsOf(user: User | null | undefined): UserProfileDetails {
  return user?.profile_details ?? {};
}

function profileFormFromUser(user: User | null | undefined): ProfileFormState {
  const details = profileDetailsOf(user);
  return {
    firstName: details.first_name ?? user?.name ?? "",
    lastName: details.last_name ?? "",
    preferredName: details.preferred_name ?? user?.name ?? "",
    username: details.username ?? "",
    role: details.role ?? "",
    profileDescription: user?.profile_description ?? "",
    phone: details.phone ?? "",
    website: websiteForForm(details.website ?? ""),
    timezone:
      user?.timezone ??
      (typeof Intl !== "undefined"
        ? Intl.DateTimeFormat().resolvedOptions().timeZone
        : ""),
    startWeek: details.start_week ?? "",
    language: user?.language ?? "en",
    timeFormat: details.time_format ?? "",
  };
}

function websiteForForm(value: string): string {
  return value.replace(/^https?:\/\//i, "");
}

function websiteForApi(value: string): string {
  const trimmed = value.trim();
  if (!trimmed || /^https?:\/\//i.test(trimmed)) return trimmed;
  return `https://${trimmed}`;
}

function profileFormsEqual(left: ProfileFormState, right: ProfileFormState) {
  return (
    left.firstName === right.firstName &&
    left.lastName === right.lastName &&
    left.preferredName === right.preferredName &&
    left.username === right.username &&
    left.role === right.role &&
    left.profileDescription === right.profileDescription &&
    left.phone === right.phone &&
    left.website === right.website &&
    left.timezone === right.timezone &&
    left.startWeek === right.startWeek &&
    left.language === right.language &&
    left.timeFormat === right.timeFormat
  );
}

function FieldLabelWithHint({
  htmlFor,
  label,
  hint,
  addon,
}: {
  htmlFor: string;
  label: string;
  hint?: string;
  addon?: ReactNode;
}) {
  return (
    <div className="flex items-center gap-1.5">
      <FieldLabel htmlFor={htmlFor}>{label}</FieldLabel>
      {hint ? (
        <Tooltip>
          <TooltipTrigger
            render={
              <button
                type="button"
                className="text-muted-foreground hover:text-foreground focus-visible:ring-ring focus-visible:ring-offset-background inline-flex rounded-sm p-0.5 transition-colors focus-visible:ring-2 focus-visible:ring-offset-2"
                aria-label={`${label} info`}
              />
            }
          >
            <InfoIcon aria-hidden="true" className="size-4" />
          </TooltipTrigger>
          <TooltipContent side="top" className="max-w-xs p-3">
            <p className="text-body leading-5">{hint}</p>
          </TooltipContent>
        </Tooltip>
      ) : null}
      {addon}
    </div>
  );
}

function CompactSelectField({
  id,
  value,
  options,
  onValueChange,
}: {
  id: string;
  value: string;
  options: SelectOption[];
  onValueChange: (value: string) => void;
}) {
  return (
    <Field className="w-full">
      <Select
        items={options}
        value={value || null}
        onValueChange={(next) => {
          if (next) onValueChange(next);
        }}
      >
        <SelectTrigger id={id} className="w-full">
          <SelectValue />
        </SelectTrigger>
        <SelectContent className="w-(--anchor-width)">
          <SelectGroup>
            {options.map((option) => (
              <SelectItem key={option.value} value={option.value}>
                {option.label}
              </SelectItem>
            ))}
          </SelectGroup>
        </SelectContent>
      </Select>
    </Field>
  );
}

function TimezoneComboboxField({
  id,
  value,
  onValueChange,
}: {
  id: string;
  value: string;
  onValueChange: (value: string) => void;
}) {
  return (
    <Field className="w-full">
      <Combobox
        items={TIMEZONE_GROUPS}
        value={value || null}
        onValueChange={(next: string | null) => {
          if (next) onValueChange(next);
        }}
      >
        <ComboboxInput
          id={id}
          placeholder="Select a timezone"
          className="w-full"
        />
        <ComboboxContent className="w-(--anchor-width) min-w-(--anchor-width)">
          <ComboboxEmpty>No timezones found.</ComboboxEmpty>
          <ComboboxList>
            {(group: TimezoneGroup) => (
              <ComboboxGroup key={group.value} items={group.items}>
                <ComboboxLabel>{group.value}</ComboboxLabel>
                <ComboboxCollection>
                  {(item: TimezoneGroup["items"][number]) => (
                    <ComboboxItem key={item.value} value={item.value}>
                      {item.label}
                    </ComboboxItem>
                  )}
                </ComboboxCollection>
              </ComboboxGroup>
            )}
          </ComboboxList>
        </ComboboxContent>
      </Combobox>
    </Field>
  );
}

export function AccountTab() {
  const { t } = useT("settings");
  const user = useAuthStore((s) => s.user);
  const localeAdapter = useLocaleAdapter();
  const setUser = useAuthStore((s) => s.setUser);
  const confirmEmailChange = useAuthStore((s) => s.confirmEmailChange);
  const [form, setForm] = useState<ProfileFormState>(() =>
    profileFormFromUser(user),
  );
  const [emailDialogOpen, setEmailDialogOpen] = useState(false);
  const [emailStep, setEmailStep] = useState<"email" | "code">("email");
  const [emailDraft, setEmailDraft] = useState(user?.email ?? "");
  const [emailCode, setEmailCode] = useState("");
  const [emailBusy, setEmailBusy] = useState(false);
  const [emailError, setEmailError] = useState<string | null>(null);

  useEffect(() => {
    setForm(profileFormFromUser(user));
    // Preserve in-progress edits when the auth store refreshes this user's
    // profile details; a user identity change is the reset boundary.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [user?.id]);

  const savedForm = useMemo(() => profileFormFromUser(user), [user]);
  const saveProfile = useCallback(
    async (next: ProfileFormState) => {
      const payload = {
        timezone: next.timezone,
        profile_description: next.profileDescription,
        profile_details: {
          first_name: next.firstName,
          last_name: next.lastName,
          preferred_name: next.preferredName,
          username: next.username,
          role: next.role,
          phone: next.phone,
          website: websiteForApi(next.website),
          start_week: next.startWeek,
          time_format: next.timeFormat,
        },
      };
      const updated = await api.updateMe(payload);
      setUser(updated);
    },
    [setUser],
  );
  const autoSave = useAutoSave({
    value: form,
    savedValue: savedForm,
    onSave: saveProfile,
    onSuccess: () =>
      toast.success(
        t(($) => $.account.toast_profile_updated),
        {
          id: "settings-auto-save",
        },
      ),
    onError: (error) =>
      toast.error(
        error instanceof Error
          ? error.message
          : t(($) => $.account.toast_profile_failed),
      ),
    enabled: !!user && !!form.firstName.trim(),
    isEqual: profileFormsEqual,
  });

  const updateField = <K extends keyof ProfileFormState>(
    key: K,
    value: ProfileFormState[K],
  ) => {
    setForm((current) => ({ ...current, [key]: value }));
  };

  const handleLanguageChange = useCallback(
    async (language: string) => {
      if (language === form.language) return;
      localeAdapter.persist(language as SupportedLocale);
      try {
        const updated = await api.updateMe({ language });
        setUser(updated);
        setForm((current) => ({ ...current, language }));
        toast.success(t(($) => $.auto_save.toast_saved), {
          id: "settings-auto-save",
        });
        // Match PreferencesTab: let the confirmation remain visible before
        // the App Router reloads with the persisted locale cookie.
        setTimeout(() => window.location.reload(), 900);
      } catch (error) {
        toast.error(
          error instanceof Error
            ? error.message
            : t(($) => $.account.toast_profile_failed),
        );
      }
    },
    [form.language, localeAdapter, setUser, t],
  );

  const handleReset = () => {
    setForm(profileFormFromUser(user));
  };

  const handleSave = async () => {
    await autoSave.flush();
  };

  const handleOpenEmailChange = () => {
    setEmailDraft(user?.email ?? "");
    setEmailCode("");
    setEmailStep("email");
    setEmailError(null);
    setEmailDialogOpen(true);
  };

  const handleRequestEmailCode = async () => {
    const email = emailDraft.trim();
    if (!email || !email.includes("@")) {
      setEmailError("Enter a valid email address.");
      return;
    }
    setEmailBusy(true);
    setEmailError(null);
    try {
      await api.requestEmailChange(email);
      setEmailStep("code");
    } catch (error) {
      setEmailError(
        error instanceof Error ? error.message : "Couldn't send a code.",
      );
    } finally {
      setEmailBusy(false);
    }
  };

  const handleConfirmEmailChange = async () => {
    if (emailCode.length !== 6) {
      setEmailError("Enter the 6-digit verification code.");
      return;
    }
    setEmailBusy(true);
    setEmailError(null);
    try {
      await confirmEmailChange(emailDraft.trim(), emailCode);
      setEmailDialogOpen(false);
      toast.success("Email updated", { id: "settings-auto-save" });
    } catch (error) {
      setEmailError(
        error instanceof Error ? error.message : "Couldn't verify the code.",
      );
    } finally {
      setEmailBusy(false);
    }
  };

  return (
    <TooltipProvider delay={200}>
      <div className="w-full max-w-4xl space-y-6">
        <Card className="overflow-hidden p-0">
          <CardContent className="px-6 py-7 sm:px-8">
            <div>
              <div className="space-y-4">
                <div className="flex flex-col gap-3 sm:flex-row sm:items-start">
                  <AvatarUploadControl
                    variant="user"
                    value={user?.avatar_url ?? null}
                    name={user?.name ?? ""}
                    size={64}
                    editBadge
                    ariaLabel={t(($) => $.account.click_avatar_hint)}
                    onUploaded={async (url) => {
                      try {
                        const updated = await api.updateMe({ avatar_url: url });
                        setUser(updated);
                        toast.success(
                          t(($) => $.account.toast_avatar_updated),
                          {
                            id: "settings-auto-save",
                          },
                        );
                      } catch (error) {
                        toast.error(
                          error instanceof Error
                            ? error.message
                            : t(($) => $.account.toast_avatar_failed),
                        );
                      }
                    }}
                  />
                  <div className="flex min-w-0 flex-1 flex-col gap-2.5">
                    <div className="min-w-0 space-y-px">
                      <h2 className="text-title-sm font-semibold tracking-tight">
                        {t(($) => $.account.avatar_label)}
                      </h2>
                      <p className="text-muted-foreground text-body">
                        {t(($) => $.account.click_avatar_hint)}
                      </p>
                    </div>
                  </div>
                </div>
              </div>

              <Separator className="my-6" />

              <div id="profile-3-basic-details" className="space-y-5">
                <SectionHeading
                  title="Basic Details"
                  description="Keep your contact and identity fields current."
                />

                <FieldGroup className="grid gap-x-6 gap-y-6 md:grid-cols-2">
                  <Field className="gap-2.5">
                    <FieldLabel htmlFor="profile-3-first-name">
                      First Name
                    </FieldLabel>
                    <Input
                      id="profile-3-first-name"
                      value={form.firstName}
                      onChange={(event) =>
                        updateField("firstName", event.target.value)
                      }
                      onBlur={() => void autoSave.flush()}
                    />
                  </Field>

                  <Field className="gap-2.5">
                    <FieldLabel htmlFor="profile-3-last-name">
                      Last Name
                    </FieldLabel>
                    <Input
                      id="profile-3-last-name"
                      value={form.lastName}
                      onChange={(event) =>
                        updateField("lastName", event.target.value)
                      }
                      onBlur={() => void autoSave.flush()}
                      placeholder="Optional"
                    />
                  </Field>

                  <Field className="gap-2.5">
                    <FieldLabelWithHint
                      htmlFor="profile-3-email"
                      label="Primary Email Address"
                      hint="This stays tied to sign-in and recovery. Use the edit action if your account allows email changes."
                      addon={
                        user && !user.is_guest ? (
                          <Badge variant="success-light" size="sm">
                            Verified
                          </Badge>
                        ) : undefined
                      }
                    />
                    <InputGroup className="w-full">
                      <InputGroupInput
                        id="profile-3-email"
                        type="email"
                        value={user?.email ?? ""}
                        readOnly
                      />
                      <InputGroupAddon align="inline-end">
                        <InputGroupButton
                          type="button"
                          variant="outline"
                          onClick={handleOpenEmailChange}
                        >
                          Edit
                        </InputGroupButton>
                      </InputGroupAddon>
                    </InputGroup>
                    <FieldDescription>
                      Used for sign-in, recovery, and workspace notices.
                    </FieldDescription>
                  </Field>

                  <Field className="gap-2.5">
                    <FieldLabelWithHint
                      htmlFor="profile-3-preferred-name"
                      label="Preferred Name"
                      hint="Shown in compact comments, activity rows, and mention previews."
                    />
                    <Input
                      id="profile-3-preferred-name"
                      value={form.preferredName}
                      onChange={(event) =>
                        updateField("preferredName", event.target.value)
                      }
                      onBlur={() => void autoSave.flush()}
                    />
                  </Field>

                  <Field className="gap-2.5">
                    <FieldLabelWithHint
                      htmlFor="profile-3-username"
                      label="Username"
                      hint="A unique profile handle saved with your account."
                    />
                    <InputGroup className="w-full">
                      <InputGroupAddon align="inline-start">
                        <InputGroupText>@</InputGroupText>
                      </InputGroupAddon>
                      <InputGroupInput
                        id="profile-3-username"
                        value={form.username}
                        onChange={(event) =>
                          updateField("username", event.target.value)
                        }
                        onBlur={() => void autoSave.flush()}
                      />
                    </InputGroup>
                  </Field>

                  <Field className="gap-2.5">
                    <FieldLabel htmlFor="profile-3-role">Role</FieldLabel>
                    <CompactSelectField
                      id="profile-3-role"
                      options={ROLE_OPTIONS}
                      value={form.role}
                      onValueChange={(value) => updateField("role", value)}
                    />
                  </Field>

                  <Field className="gap-2.5 md:col-span-2">
                    <FieldLabel htmlFor="profile-3-description">
                      About You
                    </FieldLabel>
                    <Textarea
                      id="profile-3-description"
                      value={form.profileDescription}
                      maxLength={2000}
                      placeholder="Tell agents and teammates how you work."
                      onChange={(event) =>
                        updateField("profileDescription", event.target.value)
                      }
                      onBlur={() => void autoSave.flush()}
                    />
                    <FieldDescription>
                      Shared with agents as durable requester context.
                    </FieldDescription>
                  </Field>

                  <Field className="gap-2.5">
                    <FieldLabel htmlFor="profile-3-phone">
                      Phone Number
                    </FieldLabel>
                    <PhoneInput
                      id="profile-3-phone"
                      value={form.phone || undefined}
                      onChange={(value) => updateField("phone", value || "")}
                      defaultCountry="US"
                    />
                    <FieldDescription>
                      Saved with your account profile.
                    </FieldDescription>
                  </Field>

                  <Field className="gap-2.5">
                    <FieldLabel htmlFor="profile-3-website">Website</FieldLabel>
                    <InputGroup className="w-full">
                      <InputGroupAddon align="inline-start">
                        <InputGroupText>https://</InputGroupText>
                      </InputGroupAddon>
                      <InputGroupInput
                        id="profile-3-website"
                        value={form.website}
                        onChange={(event) =>
                          updateField("website", event.target.value)
                        }
                        onBlur={() => void autoSave.flush()}
                      />
                    </InputGroup>
                  </Field>
                </FieldGroup>
              </div>

              <Separator className="my-6" />

              <div className="space-y-5">
                <SectionHeading
                  title="Regional Preferences"
                  description="Choose how time and scheduling fields appear."
                />

                <FieldGroup className="grid gap-x-6 gap-y-6 md:grid-cols-2">
                  <Field className="gap-2.5">
                    <FieldLabelWithHint
                      htmlFor="profile-3-timezone"
                      label="Preferred Timezone"
                      hint="Used for due times, reminder delivery, and schedule previews."
                    />
                    <TimezoneComboboxField
                      id="profile-3-timezone"
                      value={form.timezone}
                      onValueChange={(value) => updateField("timezone", value)}
                    />
                  </Field>

                  <Field className="gap-2.5">
                    <FieldLabelWithHint
                      htmlFor="profile-3-start-week"
                      label="Start Week On"
                      hint="Saved as your preferred first day of the week."
                    />
                    <CompactSelectField
                      id="profile-3-start-week"
                      options={START_WEEK_OPTIONS}
                      value={form.startWeek}
                      onValueChange={(value) => updateField("startWeek", value)}
                    />
                  </Field>

                  <Field className="gap-2.5">
                    <FieldLabel htmlFor="profile-3-language">
                      Language
                    </FieldLabel>
                    <CompactSelectField
                      id="profile-3-language"
                      options={LANGUAGE_OPTIONS}
                      value={form.language}
                      onValueChange={(value) => void handleLanguageChange(value)}
                    />
                  </Field>

                  <Field className="gap-2.5">
                    <FieldLabel htmlFor="profile-3-time-format">
                      Time Format
                    </FieldLabel>
                    <CompactSelectField
                      id="profile-3-time-format"
                      options={TIME_FORMAT_OPTIONS}
                      value={form.timeFormat}
                      onValueChange={(value) =>
                        updateField("timeFormat", value)
                      }
                    />
                  </Field>
                </FieldGroup>
              </div>
            </div>
          </CardContent>

          <CardFooter className="border-t px-6 py-4 sm:px-8">
            <div className="flex w-full flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
              <div className="flex min-w-0 flex-1 items-center gap-3">
                <p className="text-muted-foreground text-body">
                  Save once to update your shared profile across every
                  workspace.
                </p>
                <SettingsSaveState
                  status={autoSave.status}
                  savingLabel={t(($) => $.auto_save.saving)}
                  savedLabel={t(($) => $.auto_save.saved)}
                  errorLabel={t(($) => $.auto_save.failed)}
                />
              </div>

              <div className="flex items-center gap-2">
                <Button type="button" variant="outline" onClick={handleReset}>
                  Reset
                </Button>
                <Button type="button" onClick={() => void handleSave()}>
                  Save Changes
                </Button>
              </div>
            </div>
          </CardFooter>
        </Card>

        <AlertDialog
          open={emailDialogOpen}
          onOpenChange={(open) => {
            if (!emailBusy) setEmailDialogOpen(open);
          }}
        >
          <AlertDialogContent>
            <AlertDialogHeader>
              <AlertDialogTitle>Change email address</AlertDialogTitle>
              <AlertDialogDescription>
                {emailStep === "email"
                  ? "We'll send a verification code to the new address."
                  : `Enter the 6-digit code sent to ${emailDraft.trim()}.`}
              </AlertDialogDescription>
            </AlertDialogHeader>

            {emailStep === "email" ? (
              <Field>
                <FieldLabel htmlFor="profile-email-change">
                  New email address
                </FieldLabel>
                <Input
                  id="profile-email-change"
                  type="email"
                  autoComplete="email"
                  value={emailDraft}
                  onChange={(event) => setEmailDraft(event.target.value)}
                  disabled={emailBusy}
                />
              </Field>
            ) : (
              <div className="flex justify-center">
                <InputOTP
                  autoFocus
                  maxLength={6}
                  value={emailCode}
                  onChange={setEmailCode}
                  disabled={emailBusy}
                  aria-label="Verification code"
                >
                  <InputOTPGroup>
                    <InputOTPSlot index={0} />
                    <InputOTPSlot index={1} />
                    <InputOTPSlot index={2} />
                    <InputOTPSlot index={3} />
                    <InputOTPSlot index={4} />
                    <InputOTPSlot index={5} />
                  </InputOTPGroup>
                </InputOTP>
              </div>
            )}

            {emailError ? (
              <p className="text-body text-destructive" role="alert">
                {emailError}
              </p>
            ) : null}

            <AlertDialogFooter>
              <AlertDialogCancel disabled={emailBusy}>Cancel</AlertDialogCancel>
              <AlertDialogAction
                disabled={emailBusy}
                onClick={async (event) => {
                  event.preventDefault();
                  if (emailStep === "email") {
                    await handleRequestEmailCode();
                  } else {
                    await handleConfirmEmailChange();
                  }
                }}
              >
                {emailBusy
                  ? emailStep === "email"
                    ? "Sending..."
                    : "Verifying..."
                  : emailStep === "email"
                    ? "Send code"
                    : "Verify email"}
              </AlertDialogAction>
            </AlertDialogFooter>
          </AlertDialogContent>
        </AlertDialog>
      </div>
    </TooltipProvider>
  );
}

function SectionHeading({
  title,
  description,
}: {
  title: string;
  description: string;
}) {
  return (
    <div className="space-y-1">
      <h2 className="text-title-sm font-semibold tracking-tight">{title}</h2>
      <p className="text-muted-foreground text-body leading-relaxed">
        {description}
      </p>
    </div>
  );
}
