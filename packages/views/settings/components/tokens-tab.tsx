"use client";

import { useEffect, useState, useCallback } from "react";
import { Check, Copy } from "lucide-react";
import {
  Alert,
  Button,
  Checkbox,
  Input,
  Modal,
  Skeleton,
  Tooltip,
} from "@lobehub/ui/base-ui";
import type { PersonalAccessToken } from "@orvilo/core/types";
import { copyText } from "@orvilo/ui/lib/clipboard";
import { toast } from "sonner";
import { api } from "@orvilo/core/api";
import { useLocale, useT } from "../../i18n";
import { SettingsEmptyState } from "./settings-empty";
import { SettingsFormRow, SettingsGroup } from "./settings-shell";
import { SettingsSelect } from "./settings-select";
import { useSettingsConfirm } from "./settings-confirm";

/**
 * Authorized clients — the personal access token list.
 *
 * Two groups: the create row, and the list of issued tokens. Nothing here is an
 * antd `Form` field — the name input is a local draft and the expiry select
 * writes that draft — so no control carries a `name` and the shell's rows are
 * layout only.
 *
 * **The revoke confirmation is `useSettingsConfirm`, and the promise it returns
 * is load-bearing.** The old `AlertDialog` closed from `onSuccess`; the
 * imperative modal closes on the line after `onOk` unless `onOk` returns a
 * promise, so `handleRevokeToken` deliberately **rethrows**: a failed revoke
 * leaves the dialog open on top of the toast rather than dismissing as if the
 * token were gone.
 *
 * The security note and the tab description used to be handed to `SettingsTab`,
 * which renders `children` and nothing else whenever it sits inside the settings
 * dialog — so inside the dialog, which is the only surface this tab has, both
 * sentences were invisible. The note now sits on the create group, where it
 * renders.
 */

const EXPIRY_KEYS = ["30", "90", "365", "never"] as const;

export function TokensTab() {
  const { t } = useT("settings");
  const locale = useLocale();
  const confirm = useSettingsConfirm();
  const expiryItems = EXPIRY_KEYS.map((value) => ({
    value,
    label: t(($) => $.tokens.expiry[value]),
  }));
  const [tokens, setTokens] = useState<PersonalAccessToken[]>([]);
  const [tokenName, setTokenName] = useState("");
  const [tokenExpiry, setTokenExpiry] = useState("90");
  const [tokenCreating, setTokenCreating] = useState(false);
  const [newToken, setNewToken] = useState<string | null>(null);
  const [tokenCopied, setTokenCopied] = useState(false);
  const [commandCopied, setCommandCopied] = useState(false);
  const [storedConfirmed, setStoredConfirmed] = useState(false);
  const [tokenRevoking, setTokenRevoking] = useState<string | null>(null);
  const [tokensLoading, setTokensLoading] = useState(true);
  const [tokensLoadFailed, setTokensLoadFailed] = useState(false);

  const loadTokens = useCallback(async () => {
    try {
      const list = await api.listPersonalAccessTokens();
      setTokens(list);
      setTokensLoadFailed(false);
    } catch (e) {
      setTokensLoadFailed(true);
      toast.error(e instanceof Error ? e.message : t(($) => $.tokens.toast_load_failed));
    } finally {
      setTokensLoading(false);
    }
  }, [t]);

  useEffect(() => { loadTokens(); }, [loadTokens]);

  const handleCreateToken = async () => {
    setTokenCreating(true);
    try {
      const expiresInDays = tokenExpiry === "never" ? undefined : Number(tokenExpiry);
      const result = await api.createPersonalAccessToken({ name: tokenName, expires_in_days: expiresInDays });
      setNewToken(result.token);
      setTokenName("");
      setTokenExpiry("90");
      await loadTokens();
    } catch (e) {
      toast.error(e instanceof Error ? e.message : t(($) => $.tokens.toast_create_failed));
    } finally {
      setTokenCreating(false);
    }
  };

  /**
   * Rejects on failure on purpose: `useSettingsConfirm`'s `onOk` keeps the
   * dialog open only while its promise is unsettled, so swallowing the error
   * here would dismiss the confirmation exactly when the token still exists.
   */
  const handleRevokeToken = async (id: string) => {
    setTokenRevoking(id);
    try {
      await api.revokePersonalAccessToken(id);
      await loadTokens();
      toast.success(t(($) => $.tokens.toast_revoked));
    } catch (e) {
      toast.error(e instanceof Error ? e.message : t(($) => $.tokens.toast_revoke_failed));
      throw e;
    } finally {
      setTokenRevoking(null);
    }
  };

  const handleCopyToken = async () => {
    if (!newToken) return;
    if (await copyText(newToken)) {
      setTokenCopied(true);
      setTimeout(() => setTokenCopied(false), 2000);
    }
  };

  const handleCopyCommand = async () => {
    if (!newToken) return;
    if (await copyText(`orvilo login --token ${newToken}`)) {
      setCommandCopied(true);
      setTimeout(() => setCommandCopied(false), 2000);
    }
  };

  const closeCreatedDialog = () => {
    setNewToken(null);
    setTokenCopied(false);
    setCommandCopied(false);
    setStoredConfirmed(false);
  };

  const openRevokeConfirm = (token: PersonalAccessToken) => {
    confirm({
      title: t(($) => $.tokens.revoke_dialog.title),
      description: t(($) => $.tokens.revoke_dialog.description),
      confirmLabel: t(($) => $.tokens.revoke_dialog.confirm),
      cancelLabel: t(($) => $.tokens.revoke_dialog.cancel),
      onConfirm: () => handleRevokeToken(token.id),
    });
  };

  return (
    <>
      {/* One card rather than the old two. A `Form.Group` always renders a
          header, so a second group holding only the list had a header with no
          title in it — measured, ~59px of dead space above the first token —
          and, because `SettingsGroup` only emits `role="group"` when it has a
          name, the list had no section semantics either.

          The title is `section_title`, not `title`: `tokens.title` is verbatim
          the string the rail and the `DialogHeader` already show, and the two
          sat about 100px apart. R4 governs the *page* title and R31 the
          *section* name, so there is no conflict to resolve here — there was a
          missing section label, which is what the other tabs have
          (`notifications.title` is "Inbox Notifications" under a page called
          "Notifications"). */}
      <SettingsGroup
        variant="outlined"
        title={t(($) => $.tokens.section_title)}
        description={`${t(($) => $.tokens.description)} ${t(($) => $.tokens.security_note)}`}
      >
        <div className="flex flex-col gap-2 sm:flex-row sm:items-center">
          <Input
            aria-label={t(($) => $.tokens.name_placeholder)}
            autoComplete="off"
            className="flex-1"
            placeholder={t(($) => $.tokens.name_placeholder)}
            type="text"
            value={tokenName}
            onChange={(e) => setTokenName(e.target.value)}
          />
          <SettingsSelect
            className="w-full sm:w-32"
            id="token-expiry"
            label={t(($) => $.tokens.expiry_label)}
            options={expiryItems}
            value={tokenExpiry}
            onValueChange={setTokenExpiry}
          />
          <Button
            disabled={tokenCreating || !tokenName.trim()}
            shape="round"
            type="primary"
            onClick={handleCreateToken}
          >
            {tokenCreating ? t(($) => $.tokens.creating) : t(($) => $.tokens.create)}
          </Button>
        </div>

        {tokensLoading ? (
          Array.from({ length: 2 }).map((_, i) => (
            <SettingsFormRow key={i} divider label={<Skeleton height={16} width={128} />}>
              <Skeleton height={32} radius={8} width={80} />
            </SettingsFormRow>
          ))
        ) : tokens.length === 0 ? (
          <SettingsEmptyState
            title={tokensLoadFailed ? t(($) => $.tokens.load_failed) : t(($) => $.tokens.empty)}
          />
        ) : (
          tokens.map((token) => (
            <SettingsFormRow
              key={token.id}
              divider
              label={token.name}
              description={
                <>
                  {t(($) => $.tokens.metadata_prefix, {
                    prefix: token.token_prefix,
                    created: new Date(token.created_at).toLocaleDateString(locale),
                    lastUsed: token.last_used_at
                      ? t(($) => $.tokens.last_used_with_date, {
                          date: new Date(token.last_used_at!).toLocaleDateString(locale),
                        })
                      : t(($) => $.tokens.last_used_never),
                  })}
                  {token.expires_at && t(($) => $.tokens.expires_with_date, {
                    date: new Date(token.expires_at!).toLocaleDateString(locale),
                  })}
                </>
              }
            >
              <Button
                danger
                aria-label={t(($) => $.tokens.revoke_aria, { name: token.name })}
                disabled={tokenRevoking === token.id}
                shape="round"
                type="fill"
                onClick={() => openRevokeConfirm(token)}
              >
                {t(($) => $.tokens.revoke_tooltip)}
              </Button>
            </SettingsFormRow>
          ))
        )}
      </SettingsGroup>

      <Modal
        open={!!newToken}
        title={t(($) => $.tokens.created_dialog.title)}
        width={560}
        footer={
          <div className="flex items-center justify-between gap-3">
            <Checkbox
              checked={storedConfirmed}
              onChange={(checked) => setStoredConfirmed(checked)}
            >
              {t(($) => $.tokens.created_dialog.confirm_stored)}
            </Checkbox>
            <Button
              disabled={!storedConfirmed}
              shape="round"
              type="primary"
              onClick={closeCreatedDialog}
            >
              {t(($) => $.tokens.created_dialog.done)}
            </Button>
          </div>
        }
        onCancel={closeCreatedDialog}
      >
        <div className="flex flex-col gap-4">
          <Alert
            showIcon
            title={
              <>
                {t(($) => $.tokens.created_dialog.warning_prefix)}
                <span className="font-medium">
                  {t(($) => $.tokens.created_dialog.warning_emphasis)}
                </span>
                {t(($) => $.tokens.created_dialog.warning_suffix)}
              </>
            }
            type="warning"
          />

          <div className="flex min-w-0 items-center gap-2">
            <code className="min-w-0 flex-1 truncate rounded-md border bg-muted/50 px-3 py-2 text-body select-all">
              {newToken}
            </code>
            <Tooltip title={t(($) => $.tokens.created_dialog.copy_tooltip)}>
              {/* The icon goes in `icon`, not in `children`. Lobe sizes an
                  icon-only button (`width: 32px`, `padding-inline: 0`) only
                  when `icon` or `loading` is set and there are no children,
                  and `shape="circle"` on its own just zeroes the padding — so a
                  child-rendered icon measures 18px wide: an oval, not a
                  circle. Measured in the renderer. */}
              <Button
                aria-label={t(($) => $.tokens.created_dialog.copy_tooltip)}
                icon={tokenCopied ? Check : Copy}
                shape="circle"
                type="fill"
                onClick={handleCopyToken}
              />
            </Tooltip>
          </div>

          <div className="min-w-0 space-y-1.5">
            <p className="text-caption text-muted-foreground">
              {t(($) => $.tokens.created_dialog.cli_hint)}
            </p>
            <div className="flex min-w-0 items-center gap-2">
              <code className="min-w-0 flex-1 truncate rounded-md border bg-muted/50 px-3 py-2 text-body select-all">
                {`orvilo login --token ${newToken}`}
              </code>
              <Tooltip title={t(($) => $.tokens.created_dialog.copy_command_tooltip)}>
                <Button
                  aria-label={t(($) => $.tokens.created_dialog.copy_command_tooltip)}
                  icon={commandCopied ? Check : Copy}
                  shape="circle"
                  type="fill"
                  onClick={handleCopyCommand}
                />
              </Tooltip>
            </div>
          </div>
        </div>
      </Modal>
    </>
  );
}
