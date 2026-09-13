/* eslint-disable i18next/no-literal-string -- profile-3 is a registry block with intentional English copy; localize this data layer without changing its layout. */
/* eslint-disable no-restricted-syntax -- the copied profile-3 labels and technical control text remain literal until the settings copy is localized. */

"use client";

import {
  useCallback,
  useEffect,
  useMemo,
  useState,
} from "react";
import { Button, Input, InputOTP, Modal, Tag, TextArea } from "@lobehub/ui/base-ui";
import { PhoneInput } from "@orvilo/ui/components/reui/phone-input";
import { toast } from "sonner";
import { useAuthStore } from "@orvilo/core/auth";
import { api } from "@orvilo/core/api";
import type { SupportedLocale } from "@orvilo/core/i18n";
import { useLocaleAdapter } from "@orvilo/core/i18n/react";
import type { User, UserProfileDetails } from "@orvilo/core/types";
import { AvatarUploadControl } from "../../common/avatar-upload-control";
import { useT } from "../../i18n";
import { SettingsSaveState } from "./settings-save-state";
import { SettingsFormRow, SettingsGroup } from "./settings-shell";
import { SettingsSelect } from "./settings-select";
import { useAutoSave } from "./use-auto-save";

/**
 * Account — the profile-3 block: avatar, identity fields, contact fields and
 * regional preferences, all committed through `useAutoSave`.
 *
 * **Every control here is a draft, and the hook owns the commit.** The tab
 * keeps its page-wide `useState<ProfileFormState>` and hands it to `useAutoSave`
 * exactly as before the migration; nothing about the state model changed. What
 * that buys is the whole reason this tab is the risky one: antd's `Form` is
 * uncontrolled (`initialValues` is read once, and the only way back in is
 * `setFieldsValue`, which clobbers whatever the user is typing), so letting a
 * `Form` hold these drafts would have to solve two inverse problems at once —
 * server data arriving late and never reaching the form, and a server value
 * arriving mid-typing and overwriting the draft. `useAutoSave` already solves
 * both: `savedValue` feeds `persistedRef` through `observedSavedRef` rather than
 * through the draft, and the draft resets only on an identity change
 * (`[user?.id]` below). So **no field here carries a `name`** and none of them
 * live in a `Form` store; the shell's `Form.Group` / `Form.Item` pair is layout
 * only. See the state-ownership rule in `reference-lobe-tab-migration.md`.
 *
 * The one control that is *not* a draft is the language select: it persists the
 * locale cookie, PATCHes `/api/me` and reloads immediately, so it neither reads
 * nor writes `form.language` on its way through — it only mirrors the value the
 * server confirmed.
 *
 * `htmlFor` rather than `aria-label` on the text fields: the row's label is a
 * real `<label>` element, and antd only gives it a `for` when the item has a
 * `name`. See `SettingsFormRow`'s note on the prop.
 *
 * The title and description the old `SettingsTab` rendered are gone: the
 * settings dialog's own header renders them from this tab's entry in
 * `settings-page.tsx`.
 */

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
}

/**
 * The old `SettingsRow`'s width tiers, in pixels: a row is either hugging its
 * control or exactly this wide, which is why the text rows share one value and
 * cannot drift apart.
 *
 * The exactness comes from `base.css`, not from the prop — `Form.Item` turns
 * `minWidth` into `width`, and a flex item's `width` is only its base size.
 * `SettingsFormRow`'s `minWidth` documents both halves.
 */
const TEXT_MIN_WIDTH = 384;
const SELECT_MIN_WIDTH = 192;
const WIDE_SELECT_MIN_WIDTH = 288;

const ROLE_OPTIONS: SelectOption[] = [
  { value: "product-ops", label: "Product Operations" },
  { value: "growth-lead", label: "Growth Lead" },
  { value: "customer-ops", label: "Customer Operations" },
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

/**
 * The registry block's curated zones, flattened.
 *
 * Three things the old `TimezoneComboboxField` had are not here, and they are
 * named rather than waved at:
 *
 *   - the **group labels** ("Americas" / "Europe" / "Asia Pacific") — a flat
 *     15-item list needs no headings, and `SettingsSelect` has no grouped mode;
 *   - the **"No timezones found."** empty state — with 15 options and a filter
 *     that matches both the offset label and the IANA value, an empty result is
 *     reachable but not worth a translated string of its own;
 *   - the **placeholder** ("Select a timezone").
 *
 * The **filter itself is back**, on `search` — that is the capability the
 * combobox had and this row must keep, since scanning 15 items for one zone is
 * the interaction. The option labels, which carry the GMT offset and are what a
 * user actually reads, are unchanged.
 */
const TIMEZONE_OPTIONS: SelectOption[] = [
  { value: "America/Los_Angeles", label: "(GMT-8) Los Angeles" },
  { value: "America/Denver", label: "(GMT-7) Denver" },
  { value: "America/Chicago", label: "(GMT-6) Chicago" },
  { value: "America/New_York", label: "(GMT-5) New York" },
  { value: "America/Toronto", label: "(GMT-5) Toronto" },
  { value: "Europe/London", label: "(GMT+0) London" },
  { value: "Europe/Berlin", label: "(GMT+1) Berlin" },
  { value: "Europe/Paris", label: "(GMT+1) Paris" },
  { value: "Europe/Amsterdam", label: "(GMT+1) Amsterdam" },
  { value: "Europe/Athens", label: "(GMT+2) Athens" },
  { value: "Asia/Dubai", label: "(GMT+4) Dubai" },
  { value: "Asia/Tashkent", label: "(GMT+5) Tashkent" },
  { value: "Asia/Singapore", label: "(GMT+8) Singapore" },
  { value: "Asia/Tokyo", label: "(GMT+9) Tokyo" },
  { value: "Australia/Sydney", label: "(GMT+11) Sydney" },
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

  // The dialog is dismissable only while no request is in flight: the whole
  // flow is a two-step server conversation, and closing it mid-request would
  // strand the code the server already sent. `Modal`'s X, Escape and backdrop
  // all funnel through `onCancel`, so one guard covers all three.
  const handleEmailDialogCancel = () => {
    if (!emailBusy) setEmailDialogOpen(false);
  };

  return (
    <div className="w-full space-y-6">
      <SettingsGroup
        title="Basic Details"
        description="Keep your contact and identity fields current."
      >
        <SettingsFormRow
          label={t(($) => $.account.avatar_label)}
          description={t(($) => $.account.click_avatar_hint)}
        >
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
        </SettingsFormRow>

        <SettingsFormRow
          label="First Name"
          htmlFor="profile-3-first-name"
          minWidth={TEXT_MIN_WIDTH}
        >
          <Input
            id="profile-3-first-name"
            value={form.firstName}
            onChange={(event) =>
              updateField("firstName", event.target.value)
            }
            onBlur={() => void autoSave.flush()}
          />
        </SettingsFormRow>

        <SettingsFormRow
          label="Last Name"
          htmlFor="profile-3-last-name"
          minWidth={TEXT_MIN_WIDTH}
        >
          <Input
            id="profile-3-last-name"
            value={form.lastName}
            placeholder="Optional"
            onChange={(event) => updateField("lastName", event.target.value)}
            onBlur={() => void autoSave.flush()}
          />
        </SettingsFormRow>

        <SettingsFormRow
          label={
            <span className="inline-flex items-center gap-1.5">
              Primary Email Address
              {user && !user.is_guest ? (
                <Tag color="success" size="small" variant="filled">
                  Verified
                </Tag>
              ) : null}
            </span>
          }
          description="This stays tied to sign-in and recovery. Use the edit action if your account allows email changes."
          htmlFor="profile-3-email"
          minWidth={TEXT_MIN_WIDTH}
        >
          <Input
            id="profile-3-email"
            readOnly
            type="email"
            value={user?.email ?? ""}
            suffix={
              <Button size="small" shape="round" type="fill" onClick={handleOpenEmailChange}>
                Edit
              </Button>
            }
          />
        </SettingsFormRow>

        <SettingsFormRow
          label="Preferred Name"
          description="Shown in compact comments, activity rows, and mention previews."
          htmlFor="profile-3-preferred-name"
          minWidth={TEXT_MIN_WIDTH}
        >
          <Input
            id="profile-3-preferred-name"
            value={form.preferredName}
            onChange={(event) =>
              updateField("preferredName", event.target.value)
            }
            onBlur={() => void autoSave.flush()}
          />
        </SettingsFormRow>

        <SettingsFormRow
          label="Username"
          description="A unique profile handle saved with your account."
          htmlFor="profile-3-username"
          minWidth={TEXT_MIN_WIDTH}
        >
          <Input
            id="profile-3-username"
            prefix={<span className="text-muted-foreground">@</span>}
            value={form.username}
            onChange={(event) => updateField("username", event.target.value)}
            onBlur={() => void autoSave.flush()}
          />
        </SettingsFormRow>

        <SettingsFormRow label="Role" minWidth={SELECT_MIN_WIDTH}>
          <SettingsSelect
            className="w-full"
            id="profile-3-role"
            label="Role"
            options={ROLE_OPTIONS}
            value={form.role}
            onValueChange={(value) => updateField("role", value)}
          />
        </SettingsFormRow>

        <SettingsFormRow
          label="About You"
          description="Shared with agents as durable requester context."
          htmlFor="profile-3-description"
          minWidth={TEXT_MIN_WIDTH}
        >
          <TextArea
            id="profile-3-description"
            maxLength={2000}
            placeholder="Tell agents and teammates how you work."
            value={form.profileDescription}
            onChange={(event) =>
              updateField("profileDescription", event.target.value)
            }
            onBlur={() => void autoSave.flush()}
          />
        </SettingsFormRow>

        <SettingsFormRow
          label="Phone Number"
          description="Saved with your account profile."
          htmlFor="profile-3-phone"
          minWidth={TEXT_MIN_WIDTH}
        >
          <PhoneInput
            id="profile-3-phone"
            value={form.phone || undefined}
            onChange={(value) => updateField("phone", value || "")}
            defaultCountry="US"
          />
        </SettingsFormRow>

        <SettingsFormRow
          label="Website"
          htmlFor="profile-3-website"
          minWidth={TEXT_MIN_WIDTH}
        >
          <Input
            id="profile-3-website"
            prefix={<span className="text-muted-foreground">https://</span>}
            value={form.website}
            onChange={(event) => updateField("website", event.target.value)}
            onBlur={() => void autoSave.flush()}
          />
        </SettingsFormRow>
      </SettingsGroup>

      <SettingsGroup
        title="Regional Preferences"
        description="Choose how time and scheduling fields appear."
      >
        <SettingsFormRow
          label="Preferred Timezone"
          description="Used for due times, reminder delivery, and schedule previews."
          minWidth={WIDE_SELECT_MIN_WIDTH}
        >
          <SettingsSelect
            className="w-full"
            id="profile-3-timezone"
            label="Preferred Timezone"
            options={TIMEZONE_OPTIONS}
            search
            value={form.timezone}
            onValueChange={(value) => updateField("timezone", value)}
          />
        </SettingsFormRow>

        <SettingsFormRow
          label="Start Week On"
          description="Saved as your preferred first day of the week."
          minWidth={SELECT_MIN_WIDTH}
        >
          <SettingsSelect
            className="w-full"
            id="profile-3-start-week"
            label="Start Week On"
            options={START_WEEK_OPTIONS}
            value={form.startWeek}
            onValueChange={(value) => updateField("startWeek", value)}
          />
        </SettingsFormRow>

        <SettingsFormRow label="Language" minWidth={SELECT_MIN_WIDTH}>
          <SettingsSelect
            className="w-full"
            id="profile-3-language"
            label="Language"
            options={LANGUAGE_OPTIONS}
            value={form.language}
            onValueChange={(value) => void handleLanguageChange(value)}
          />
        </SettingsFormRow>

        <SettingsFormRow label="Time Format" minWidth={SELECT_MIN_WIDTH}>
          <SettingsSelect
            className="w-full"
            id="profile-3-time-format"
            label="Time Format"
            options={TIME_FORMAT_OPTIONS}
            value={form.timeFormat}
            onValueChange={(value) => updateField("timeFormat", value)}
          />
        </SettingsFormRow>
      </SettingsGroup>

      {/* No page-level save exists — the footer flushes the debounce that
          `useAutoSave` already owns, which is why "Save Changes" is a real
          action rather than a relabelled Close. */}
      <div className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
        <div className="flex min-w-0 flex-1 items-center gap-3">
          <p className="text-body text-muted-foreground">
            Save once to update your shared profile across every workspace.
          </p>
          <SettingsSaveState
            status={autoSave.status}
            savingLabel={t(($) => $.auto_save.saving)}
            savedLabel={t(($) => $.auto_save.saved)}
            errorLabel={t(($) => $.auto_save.failed)}
          />
        </div>

        <div className="flex items-center gap-2">
          <Button shape="round" type="fill" onClick={handleReset}>
            Reset
          </Button>
          <Button
            shape="round"
            type="primary"
            onClick={() => void handleSave()}
          >
            Save Changes
          </Button>
        </div>
      </div>

      {/*
        A self-contained dialog: its values exist only while it is open and no
        server value can arrive late, which is the one place the reference says
        an antd `Form` belongs. It is not one here because the flow has **two**
        commit points, not one — "Send code" and "Verify email" are separate
        requests with separate errors — so the state stays local and owned by
        the component that runs them.
      */}
      <Modal
        keyboard={!emailBusy}
        maskClosable={!emailBusy}
        open={emailDialogOpen}
        title="Change email address"
        width={420}
        footer={
          <div className="flex items-center justify-end gap-2">
            <Button
              disabled={emailBusy}
              shape="round"
              type="fill"
              onClick={handleEmailDialogCancel}
            >
              Cancel
            </Button>
            <Button
              loading={emailBusy}
              shape="round"
              type="primary"
              onClick={() => {
                if (emailStep === "email") void handleRequestEmailCode();
                else void handleConfirmEmailChange();
              }}
            >
              {emailBusy
                ? emailStep === "email"
                  ? "Sending..."
                  : "Verifying..."
                : emailStep === "email"
                  ? "Send code"
                  : "Verify email"}
            </Button>
          </div>
        }
        onCancel={handleEmailDialogCancel}
      >
        <div className="flex flex-col gap-4">
          <p className="text-body text-muted-foreground">
            {emailStep === "email"
              ? "We'll send a verification code to the new address."
              : `Enter the 6-digit code sent to ${emailDraft.trim()}.`}
          </p>

          {emailStep === "email" ? (
            <div className="flex flex-col gap-1.5">
              <label
                className="text-body font-medium"
                htmlFor="profile-email-change"
              >
                New email address
              </label>
              <Input
                autoComplete="email"
                disabled={emailBusy}
                id="profile-email-change"
                type="email"
                value={emailDraft}
                onChange={(event) => setEmailDraft(event.target.value)}
              />
            </div>
          ) : (
            <div className="flex flex-col gap-1.5">
              {/* The id belongs on the OTP root, not on a slot: base-ui derives
                  every slot's id from it (`{id}-2`, `{id}-3`, …), so one
                  `<label for>` labels the group by labelling its first cell. */}
              <label
                className="text-body font-medium"
                htmlFor="profile-email-code"
              >
                Verification code
              </label>
              <InputOTP
                disabled={emailBusy}
                id="profile-email-code"
                length={6}
                value={emailCode}
                onChange={setEmailCode}
              />
            </div>
          )}

          {emailError ? (
            <p className="text-body text-destructive" role="alert">
              {emailError}
            </p>
          ) : null}
        </div>
      </Modal>
    </div>
  );
}
