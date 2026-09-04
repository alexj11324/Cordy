"use client";

import { useCallback, useEffect, useMemo, useState } from "react";
import { Check, Pencil } from "lucide-react";
import { Input } from "@patchbay/ui/components/ui/input";
import { Textarea } from "@patchbay/ui/components/ui/textarea";
import { cn } from "@patchbay/ui/lib/utils";
import { toast } from "sonner";
import { useAuthStore } from "@patchbay/core/auth";
import { api } from "@patchbay/core/api";
import { AvatarUploadControl } from "../../common/avatar-upload-control";
import { useT } from "../../i18n";
import {
  SETTINGS_INLINE_FIELD_CLASS,
  SettingsCard,
  SettingsPillButton,
  SettingsRow,
  SettingsSaveState,
  SettingsSection,
  SettingsTab,
} from "./settings-layout";
import { useAutoSave } from "./use-auto-save";

// Mirror server/internal/handler/auth.go:MaxProfileDescriptionLen. Counted in
// JS String.length (UTF-16 code units) here while the server counts runes,
// so a profile full of supplementary-plane emoji will trip the client cap
// before the server's — which is the safer direction of drift.
const MAX_PROFILE_DESCRIPTION_LEN = 2000;
const PROFILE_AVATAR_SIZE = 192;

interface ProfileDraft {
  name: string;
  profileDescription: string;
}

function profilesEqual(left: ProfileDraft, right: ProfileDraft) {
  return left.name === right.name && left.profileDescription === right.profileDescription;
}

export function AccountTab() {
  const { t } = useT("settings");
  const user = useAuthStore((s) => s.user);
  const setUser = useAuthStore((s) => s.setUser);

  const [profileName, setProfileName] = useState(user?.name ?? "");
  const [profileDescription, setProfileDescription] = useState(
    user?.profile_description ?? "",
  );
  const [isEditing, setIsEditing] = useState(false);

  useEffect(() => {
    setProfileName(user?.name ?? "");
    setProfileDescription(user?.profile_description ?? "");
    // Preserve in-progress edits when an avatar upload or auto-save replaces
    // the current user object in the auth store.
    // eslint-disable-next-line react-hooks/exhaustive-deps -- intentionally keyed on user identity
  }, [user?.id]);

  const descriptionTooLong = profileDescription.length > MAX_PROFILE_DESCRIPTION_LEN;

  const draft = useMemo(
    () => ({ name: profileName, profileDescription }),
    [profileDescription, profileName],
  );
  const savedDraft = useMemo(
    () => ({
      name: user?.name ?? "",
      profileDescription: user?.profile_description ?? "",
    }),
    [user?.name, user?.profile_description],
  );
  const saveProfile = useCallback(
    async (next: ProfileDraft) => {
      const updated = await api.updateMe({
        name: next.name,
        profile_description: next.profileDescription,
      });
      setUser(updated);
    },
    [setUser],
  );
  const autoSave = useAutoSave({
    value: draft,
    savedValue: savedDraft,
    onSave: saveProfile,
    onSuccess: () =>
      toast.success(t(($) => $.account.toast_profile_updated), {
        id: "settings-auto-save",
      }),
    onError: (error) =>
      toast.error(
        error instanceof Error
          ? error.message
          : t(($) => $.account.toast_profile_failed),
      ),
    enabled: isEditing && !!user && !!profileName.trim() && !descriptionTooLong,
    isEqual: profilesEqual,
  });

  const handleEditToggle = () => {
    if (!isEditing) {
      setIsEditing(true);
      return;
    }
    autoSave.flush();
    if (profileName.trim() && !descriptionTooLong) {
      setIsEditing(false);
    }
  };

  return (
    <SettingsTab
      title={t(($) => $.page.tabs.profile)}
      description={t(($) => $.account.page_description)}
    >
      <div className="flex justify-center">
        <AvatarUploadControl
          variant="user"
          value={user?.avatar_url ?? null}
          name={user?.name ?? ""}
          size={PROFILE_AVATAR_SIZE}
          editBadge
          ariaLabel={t(($) => $.account.click_avatar_hint)}
          onUploaded={async (url) => {
            try {
              const updated = await api.updateMe({ avatar_url: url });
              setUser(updated);
              toast.success(t(($) => $.account.toast_avatar_updated), {
                id: "settings-auto-save",
              });
            } catch (error) {
              toast.error(
                error instanceof Error
                  ? error.message
                  : t(($) => $.account.toast_avatar_failed),
              );
            }
          }}
        />
      </div>

      <SettingsSection
        title={t(($) => $.account.section_profile)}
        action={
          <div className="flex items-center gap-2">
            <SettingsSaveState
              status={autoSave.status}
              savingLabel={t(($) => $.auto_save.saving)}
              savedLabel={t(($) => $.auto_save.saved)}
              errorLabel={t(($) => $.auto_save.failed)}
            />
            <SettingsPillButton
              icon={isEditing ? Check : Pencil}
              active={isEditing}
              disabled={!user}
              onClick={handleEditToggle}
              aria-label={
                isEditing
                  ? t(($) => $.account.done_aria)
                  : t(($) => $.account.edit_aria)
              }
            >
              {isEditing ? t(($) => $.account.done) : t(($) => $.account.edit)}
            </SettingsPillButton>
          </div>
        }
      >
        <SettingsCard>
          <SettingsRow
            layout="stack"
            htmlFor={isEditing ? "profile-display-name" : undefined}
            label={t(($) => $.account.name_label)}
          >
            {isEditing ? (
              <Input
                id="profile-display-name"
                type="text"
                name="profile-name"
                autoComplete="name"
                autoFocus
                className={SETTINGS_INLINE_FIELD_CLASS}
                aria-label={t(($) => $.account.name_label)}
                value={profileName}
                onChange={(event) => setProfileName(event.target.value)}
                onBlur={autoSave.flush}
              />
            ) : (
              <p
                className="min-w-0 truncate text-body text-muted-foreground"
                data-testid="profile-display-name-value"
                title={profileName || t(($) => $.account.not_set)}
              >
                {profileName || t(($) => $.account.not_set)}
              </p>
            )}
          </SettingsRow>

          <SettingsRow
            layout="stack"
            htmlFor={isEditing ? "profile-about" : undefined}
            label={t(($) => $.account.profile_description_label)}
            description={t(($) => $.account.profile_description_hint)}
          >
            {isEditing ? (
              <div>
                <Textarea
                  id="profile-about"
                  name="profile-description"
                  autoComplete="off"
                  aria-label={t(($) => $.account.profile_description_label)}
                  value={profileDescription}
                  onChange={(event) => setProfileDescription(event.target.value)}
                  onBlur={autoSave.flush}
                  placeholder={t(($) => $.account.profile_description_placeholder)}
                  rows={5}
                  maxLength={MAX_PROFILE_DESCRIPTION_LEN}
                  aria-invalid={descriptionTooLong}
                  className={cn(
                    SETTINGS_INLINE_FIELD_CLASS,
                    "min-h-[72px] resize-none leading-6",
                  )}
                />
                <div className="mt-1 flex justify-end text-caption text-muted-foreground">
                  <span
                    className={descriptionTooLong ? "text-destructive shrink-0" : "shrink-0"}
                    aria-live="polite"
                  >
                    {profileDescription.length}/{MAX_PROFILE_DESCRIPTION_LEN}
                  </span>
                </div>
                {descriptionTooLong ? (
                  <p className="mt-1 text-caption text-destructive">
                    {t(($) => $.account.profile_description_too_long, {
                      max: MAX_PROFILE_DESCRIPTION_LEN,
                      count: profileDescription.length,
                    })}
                  </p>
                ) : null}
              </div>
            ) : (
              <p
                className="min-w-0 break-words text-body text-muted-foreground"
                data-testid="profile-about-value"
                title={profileDescription || t(($) => $.account.not_set)}
              >
                {profileDescription || t(($) => $.account.not_set)}
              </p>
            )}
          </SettingsRow>
        </SettingsCard>
      </SettingsSection>
    </SettingsTab>
  );
}
