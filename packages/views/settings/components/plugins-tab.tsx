"use client";

import { useMemo, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { AlertCircle, CalendarClock, Trash2, Upload } from "lucide-react";
import { toast } from "sonner";
import { Alert, Button, Input, Skeleton, TextArea } from "@lobehub/ui/base-ui";
import { useCurrentMember } from "@orvilo/core/permissions";
import {
  pluginInstallationsOptions,
  pluginPackagesOptions,
  useConfigurePlugin,
  useDeletePluginPackage,
  useInstallPlugin,
  usePreviewPlugin,
  usePublishPluginPackage,
  useSetPluginEnabled,
  useUninstallPlugin,
} from "@orvilo/core/plugins";
import { useCurrentWorkspace } from "@orvilo/core/paths";
import type {
  PluginConfigField,
  PluginInstallation,
  PluginPackage,
  PluginPreview,
} from "@orvilo/core/types";
import { Badge } from "@orvilo/ui/components/ui/badge";
import { mcpHooks, PluginHookActivity, PluginMCPApproval, PluginScheduleActivity } from "../../plugins";
import { useLocale, useT } from "../../i18n";
import { SettingsEmptyState } from "./settings-empty";
import { SettingsSelect } from "./settings-select";
import { SettingsFormRow, SettingsGroup } from "./settings-shell";
import { SettingsSwitch } from "./settings-switch";

/**
 * The scope list is the entire trust model: there is no signature, no
 * publisher verification, and no trust tier. So the consent screen shows the
 * raw scope strings alongside a plain-language line, and never summarizes them
 * away.
 */
function ScopeList({ scopes, highlighted }: { scopes: string[]; highlighted?: string[] }) {
  const { t } = useT("settings");
  const added = new Set(highlighted ?? []);
  return (
    <ul className="space-y-1.5">
      {scopes.map((scope) => (
        <li key={scope} className="flex items-baseline gap-2 text-caption">
          <code className="shrink-0 rounded bg-muted px-1.5 py-0.5 font-mono">{scope}</code>
          <span className="text-muted-foreground">{scopeDescription(scope, t)}</span>
          {added.has(scope) ? (
            <Badge variant="secondary">{t(($) => $.plugins.consent.new_scope)}</Badge>
          ) : null}
        </li>
      ))}
    </ul>
  );
}

type ScheduledHook = {
  key: string;
  name: string;
  schedule?: { cron: string; timezone: string; next_run_at?: string };
};

function ScheduleList({ hooks, showNextRun = false }: { hooks: ScheduledHook[]; showNextRun?: boolean }) {
  const { t } = useT("settings");
  const locale = useLocale();
  return (
    <ul className="mt-2 space-y-1.5">
      {hooks.map((hook) => (
        <li key={hook.key} className="flex flex-wrap items-baseline gap-2 text-caption">
          <span className="font-medium">{hook.name}</span>
          <span>{scheduleFrequency(hook.schedule?.cron ?? "", t)}</span>
          <code className="rounded bg-muted px-1.5 py-0.5 font-mono">{hook.schedule?.cron ?? ""}</code>
          <span className="text-muted-foreground">{hook.schedule?.timezone ?? ""}</span>
          {showNextRun && hook.schedule?.next_run_at ? (
            <span className="text-muted-foreground">
              {t(($) => $.plugins.schedule.next_run, {
                time: formatScheduleTime(hook.schedule?.next_run_at, locale),
              })}
            </span>
          ) : null}
        </li>
      ))}
    </ul>
  );
}

function formatScheduleTime(value: string | undefined, locale: string): string {
  if (!value) return "—";
  const timestamp = Date.parse(value);
  if (Number.isNaN(timestamp)) return "—";
  return new Intl.DateTimeFormat(locale, {
    dateStyle: "medium",
    timeStyle: "short",
  }).format(new Date(timestamp));
}

type Translate = ReturnType<typeof useT<"settings">>["t"];

function scheduleFrequency(cron: string, t: Translate): string {
  const fields = cron.trim().split(/\s+/);
  if (fields.length !== 5) return t(($) => $.plugins.schedule.frequency_custom);
  const everyMinutes = /^\*\/(\d+)$/.exec(fields[0] ?? "");
  if (everyMinutes && fields.slice(1).every((field) => field === "*")) {
    return t(($) => $.plugins.schedule.frequency_minutes, { count: Number(everyMinutes[1]) });
  }
  if (/^\d+$/.test(fields[0] ?? "") && fields.slice(1).every((field) => field === "*")) {
    return t(($) => $.plugins.schedule.frequency_hourly, { minute: fields[0] });
  }
  if (/^\d+$/.test(fields[0] ?? "") && /^\d+$/.test(fields[1] ?? "") && fields.slice(2).every((field) => field === "*")) {
    return t(($) => $.plugins.schedule.frequency_daily, {
      hour: String(fields[1]).padStart(2, "0"),
      minute: String(fields[0]).padStart(2, "0"),
    });
  }
  return t(($) => $.plugins.schedule.frequency_custom);
}

function scopeDescription(scope: string, t: Translate): string {
  if (scope.startsWith("net:")) {
    return t(($) => $.plugins.scopes.net, { domain: scope.slice("net:".length) });
  }
  switch (scope) {
    case "issues:read": return t(($) => $.plugins.scopes.issues_read);
    case "issues:write": return t(($) => $.plugins.scopes.issues_write);
    case "comments:read": return t(($) => $.plugins.scopes.comments_read);
    case "comments:write": return t(($) => $.plugins.scopes.comments_write);
    case "tasks:read": return t(($) => $.plugins.scopes.tasks_read);
    case "tasks:write": return t(($) => $.plugins.scopes.tasks_write);
    case "agents:read": return t(($) => $.plugins.scopes.agents_read);
    case "members:read": return t(($) => $.plugins.scopes.members_read);
    case "storage:user": return t(($) => $.plugins.scopes.storage_user);
    case "storage:workspace": return t(($) => $.plugins.scopes.storage_workspace);
    default: return t(($) => $.plugins.scopes.unknown);
  }
}

/**
 * The configuration form is generated from the manifest, not supplied by the
 * plugin: rendering plugin-authored form markup in the host would put plugin
 * code on our origin.
 */
function ConfigForm({
  installation,
  canManage,
  wsId,
}: {
  installation: PluginInstallation;
  canManage: boolean;
  wsId: string;
}) {
  const { t } = useT("settings");
  const configureMutation = useConfigurePlugin(wsId);
  const [values, setValues] = useState<Record<string, unknown>>(() => ({ ...installation.config }));
  const [secrets, setSecrets] = useState<Record<string, string>>({});

  if (installation.config_schema.length === 0) return null;

  const setValue = (key: string, value: unknown) => setValues((current) => ({ ...current, [key]: value }));
  const configuredSecrets = new Set(installation.configured_secrets);

  const submit = async () => {
    const payload: Record<string, unknown> = { ...values };
    // Only send a secret the admin actually typed. Sending "" would clear a
    // stored secret every time an unrelated field is saved.
    for (const [key, value] of Object.entries(secrets)) {
      if (value.length > 0) payload[key] = value;
    }
    try {
      await configureMutation.mutateAsync({ installationId: installation.id, values: payload });
      setSecrets({});
      toast.success(t(($) => $.plugins.config.saved));
    } catch (error) {
      toast.error(error instanceof Error ? error.message : t(($) => $.plugins.action_failed));
    }
  };

  // `px-4` is gone: this block is a child of an outlined `SettingsGroup`, whose
  // panel already supplies `padding-inline: 16px`. The vertical padding and the
  // top border are the old card's `divide-y` separator, kept.
  return (
    <div className="space-y-4 border-t border-surface-border py-4">
      <div className="text-caption font-medium">{t(($) => $.plugins.config.title)}</div>
      {installation.config_schema.map((field) => (
        <ConfigField
          key={field.key}
          field={field}
          value={values[field.key]}
          secretValue={secrets[field.key] ?? ""}
          secretConfigured={configuredSecrets.has(field.key)}
          disabled={!canManage || configureMutation.isPending}
          onValueChange={(value) => setValue(field.key, value)}
          onSecretChange={(value) => setSecrets((current) => ({ ...current, [field.key]: value }))}
        />
      ))}
      <div className="flex justify-end">
        {/* The baseline was a shadcn `<Button size="sm">` with no `variant` —
            solid primary, which Lobe spells `type="primary"`. The pending
            `<Loader2>` child is Lobe's own `loading` slot instead. */}
        <Button
          disabled={!canManage || configureMutation.isPending}
          loading={configureMutation.isPending}
          type="primary"
          onClick={submit}
        >
          {t(($) => $.plugins.config.save)}
        </Button>
      </div>
    </div>
  );
}

function ConfigField({
  field,
  value,
  secretValue,
  secretConfigured,
  disabled,
  onValueChange,
  onSecretChange,
}: {
  field: PluginConfigField;
  value: unknown;
  secretValue: string;
  secretConfigured: boolean;
  disabled: boolean;
  onValueChange: (value: unknown) => void;
  onSecretChange: (value: string) => void;
}) {
  const { t } = useT("settings");
  // The baseline was a hand-written two-column flex row. `SettingsFormRow`
  // carries the same split and adds `htmlFor`, which is how a bare Lobe `Input`
  // gets an accessible name: antd mints a label's `for` only from a field
  // `name`, and the state-ownership rule forbids one here. `minWidth={384}` is
  // the old `sm:w-96`.
  //
  // The switch is deliberately not given an `htmlFor`, and it is the row that
  // makes the reason worth writing down. A `for=` association *renames* its
  // control — `SettingsSwitch` renders a real `<button>`, which
  // `isLabelableElement` counts — so associating the row with a switch would
  // replace the switch's own name with the row's whole label text. The same is
  // true of the nested route (`getControlOfLabel` falls back to the first
  // labelable descendant), which this row is not on either: the switch sits in
  // the control column, a sibling of the label, not inside it.
  //
  // Measured on the rendered row: the switch's `computeAccessibleName` is
  // "Enable thing" (its own `aria-label`), `insideLabelSubtree` and
  // `namedByForLabel` both false. The enum beside it *is* on the second route
  // by design — `htmlFor` is what names a control that cannot name itself —
  // and its name is the row's label.
  const controlId = `plugin-config-${field.key}`;
  const namedControl = field.type !== "bool";

  return (
    <SettingsFormRow
      description={field.description}
      htmlFor={namedControl ? controlId : undefined}
      label={
        <span className="text-caption font-medium">
          {field.label}
          {field.required ? <span className="ml-1 text-destructive">*</span> : null}
        </span>
      }
      minWidth={384}
    >
      {field.type === "secret" ? (
        <Input
          autoComplete="off"
          disabled={disabled}
          id={controlId}
          placeholder={secretConfigured
            ? t(($) => $.plugins.config.secret_set)
            : field.placeholder ?? ""}
          type="password"
          value={secretValue}
          onChange={(event) => onSecretChange(event.target.value)}
        />
      ) : field.type === "bool" ? (
        <SettingsSwitch
          checked={value === true}
          disabled={disabled}
          label={field.label}
          onCheckedChange={(checked) => onValueChange(checked === true)}
        />
      ) : field.type === "enum" ? (
        <SettingsSelect
          className="w-full"
          disabled={disabled}
          id={controlId}
          label={field.label}
          options={(field.options ?? []).map((option) => ({ value: option, label: option }))}
          value={typeof value === "string" ? value : ""}
          onValueChange={(next) => next && onValueChange(next)}
        />
      ) : field.type === "number" ? (
        <Input
          disabled={disabled}
          id={controlId}
          placeholder={field.placeholder ?? ""}
          type="number"
          value={typeof value === "number" ? String(value) : ""}
          onChange={(event) => {
            const parsed = Number(event.target.value);
            onValueChange(event.target.value === "" || Number.isNaN(parsed) ? undefined : parsed);
          }}
        />
      ) : field.multiline === true ? (
        // A field whose value is a list of lines is unreadable in a
        // single-line input — and the generated form is the one piece of
        // plugin UI the host owns, so getting it wrong is our bug.
        <TextArea
          disabled={disabled}
          id={controlId}
          placeholder={field.placeholder ?? ""}
          rows={4}
          value={typeof value === "string" ? value : ""}
          onChange={(event) => onValueChange(event.target.value)}
        />
      ) : (
        <Input
          disabled={disabled}
          id={controlId}
          placeholder={field.placeholder ?? ""}
          value={typeof value === "string" ? value : ""}
          onChange={(event) => onValueChange(event.target.value)}
        />
      )}
    </SettingsFormRow>
  );
}

/**
 * Publishing, and installing what was published.
 *
 * These are one screen because they are two halves of one story: an author
 * uploads an artifact, and an administrator installs one specific version of it.
 * There is no way to install code that was not published here first — which is
 * what makes the consent screen below a statement about the code and not just
 * about the manifest.
 */
function PublishAndInstall({ wsId, canManage }: { wsId: string; canManage: boolean }) {
  const { t } = useT("settings");
  const { data, isLoading } = useQuery(pluginPackagesOptions(wsId));
  const publishMutation = usePublishPluginPackage(wsId);
  const deleteMutation = useDeletePluginPackage(wsId);
  const previewMutation = usePreviewPlugin(wsId);
  const installMutation = useInstallPlugin(wsId);
  const fileRef = useRef<HTMLInputElement>(null);
  const [preview, setPreview] = useState<PluginPreview | null>(null);

  const packages = useMemo(() => data?.packages ?? [], [data]);
  const scheduledHooks = (preview?.manifest.contributes?.hooks ?? [])
    .filter((hook) => hook.schedule !== undefined);

  const reportError = (error: unknown) => {
    toast.error(error instanceof Error ? error.message : t(($) => $.plugins.action_failed));
  };

  const publish = async (file: File) => {
    try {
      const published = await publishMutation.mutateAsync(file);
      toast.success(t(($) => $.plugins.publish.published, {
        name: published.name,
        version: published.versions[0]?.version ?? "",
      }));
    } catch (error) {
      reportError(error);
    } finally {
      // Cleared unconditionally so picking the same file again re-fires change,
      // which is what a person does after fixing a rejected bundle.
      if (fileRef.current) fileRef.current.value = "";
    }
  };

  const review = async (versionId: string) => {
    try {
      setPreview(await previewMutation.mutateAsync({ version_id: versionId }));
    } catch (error) {
      setPreview(null);
      reportError(error);
    }
  };

  const confirmInstall = async () => {
    if (!preview) return;
    try {
      await installMutation.mutateAsync({ version_id: preview.version_id, granted_scopes: preview.scopes });
      setPreview(null);
      toast.success(t(($) => $.plugins.consent.installed));
    } catch (error) {
      reportError(error);
    }
  };

  return (
    <SettingsGroup
      // `desc` renders as a `<small>` in the group's header, so the scale the
      // old section paragraph carried has to travel on the node itself.
      description={
        <span className="block max-w-3xl text-body leading-relaxed text-muted-foreground">
          {t(($) => $.plugins.publish.description)}
        </span>
      }
      title={t(($) => $.plugins.publish.title)}
    >
      <div className="flex flex-col gap-2 pb-4 sm:flex-row sm:items-center sm:justify-between">
        <p className="text-caption text-muted-foreground">{t(($) => $.plugins.publish.hint)}</p>
        <input
          ref={fileRef}
          type="file"
          accept=".zip,application/zip"
          className="sr-only"
          aria-label={t(($) => $.plugins.publish.upload)}
          onChange={(event) => {
            const file = event.target.files?.[0];
            if (file) void publish(file);
          }}
        />
        {/* The baseline was a shadcn `<Button>` with no `variant` — solid
            primary — and a `<Loader2>`/`<Upload>` swap while pending; Lobe
            spells both with `type="primary"` and `icon` + `loading`. */}
        <Button
          disabled={!canManage || publishMutation.isPending}
          icon={<Upload />}
          loading={publishMutation.isPending}
          type="primary"
          onClick={() => fileRef.current?.click()}
        >
          {t(($) => $.plugins.publish.upload)}
        </Button>
      </div>

      {isLoading ? (
        <div className="border-t border-surface-border py-4">
          <Skeleton height={64} aria-label={t(($) => $.plugins.loading)} />
        </div>
      ) : packages.length === 0 ? (
        <p className="border-t border-surface-border py-4 text-caption text-muted-foreground">
          {t(($) => $.plugins.publish.empty)}
        </p>
      ) : (
        packages.map((pluginPackage) => (
          <PublishedPackage
            key={pluginPackage.id}
            pluginPackage={pluginPackage}
            canManage={canManage}
            busy={previewMutation.isPending || deleteMutation.isPending}
            onReview={review}
            onDelete={(packageId) => deleteMutation
              .mutateAsync(packageId)
              .then(() => toast.success(t(($) => $.plugins.publish.deleted)))
              .catch(reportError)}
          />
        ))
      )}

      {preview ? (
        <div className="space-y-4 border-t border-surface-border py-4">
          <div>
            <div className="text-body font-semibold">{preview.manifest.name}</div>
            <p className="text-caption text-muted-foreground">
              {t(($) => $.plugins.byline, {
                author: preview.manifest.author.name,
                version: preview.version,
              })}
              {preview.installed
                ? t(($) => $.plugins.consent.upgrade_from, { version: preview.installed_version ?? "" })
                : ""}
            </p>
            {preview.manifest.description ? (
              <p className="mt-2 text-caption">{preview.manifest.description}</p>
            ) : null}
          </div>

          {/* The two consent notices were shadcn's *default* `Alert` — a
              neutral card-coloured box — so they stay on Lobe's neutral
              `secondary` tone rather than being promoted to `warning`, which
              would be a treatment change this migration did not authorize. The
              icons are the baseline ones. */}
          <Alert
            description={t(($) => $.plugins.consent.description)}
            icon={AlertCircle}
            title={t(($) => $.plugins.consent.title)}
            type="secondary"
          />

          <ScopeList scopes={preview.scopes} highlighted={preview.added_scopes} />

          {scheduledHooks.length > 0 ? (
            <Alert
              description={
                <>
                  {t(($) => $.plugins.schedule.consent_description)}
                  <ScheduleList hooks={scheduledHooks} />
                </>
              }
              icon={CalendarClock}
              title={t(($) => $.plugins.schedule.consent_title)}
              type="secondary"
            />
          ) : null}

          <div className="flex justify-end gap-2">
            {/* `variant="ghost"` is `type="text"`, the row this file's mapping
                table carries for the ghost treatment. */}
            <Button type="text" onClick={() => setPreview(null)}>
              {t(($) => $.plugins.consent.cancel)}
            </Button>
            <Button
              disabled={!canManage || installMutation.isPending}
              loading={installMutation.isPending}
              type="primary"
              onClick={confirmInstall}
            >
              {preview.installed
                ? t(($) => $.plugins.consent.confirm_upgrade)
                : t(($) => $.plugins.consent.confirm)}
            </Button>
          </div>
        </div>
      ) : null}
    </SettingsGroup>
  );
}

function PublishedPackage({
  pluginPackage,
  canManage,
  busy,
  onReview,
  onDelete,
}: {
  pluginPackage: PluginPackage;
  canManage: boolean;
  busy: boolean;
  onReview: (versionId: string) => void;
  onDelete: (packageId: string) => void;
}) {
  const { t } = useT("settings");
  // Versions arrive newest first, and the installed one is marked rather than
  // inferred: after a publish those are different rows, and that difference is
  // the whole reason an upgrade is a decision somebody makes.
  const installed = pluginPackage.versions.find((version) => version.installed === true);

  return (
    <div className="space-y-3 border-t border-surface-border py-4">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="min-w-0">
          <div className="text-body font-medium">{pluginPackage.name}</div>
          <p className="text-caption text-muted-foreground">{pluginPackage.plugin_key}</p>
        </div>
        {/* `variant="ghost" size="icon"` is Lobe's `type="text"` with the icon
            as the only child, which selects its icon-only geometry. */}
        <Button
          aria-label={t(($) => $.plugins.publish.delete)}
          disabled={!canManage || busy}
          icon={<Trash2 />}
          type="text"
          onClick={() => onDelete(pluginPackage.id)}
        />
      </div>

      <ul className="space-y-1.5">
        {pluginPackage.versions.map((version) => (
          <li key={version.id} className="flex flex-wrap items-center gap-2 text-caption">
            <code className="rounded bg-muted px-1.5 py-0.5 font-mono">{version.version}</code>
            {version.installed === true ? (
              <Badge variant="secondary">{t(($) => $.plugins.publish.installed_version)}</Badge>
            ) : null}
            <span className="text-muted-foreground">{version.published_at.slice(0, 10)}</span>
            <span className="font-mono text-muted-foreground" title={version.digest}>
              {version.digest.slice(0, 12)}
            </span>
            {version.installed === true ? null : (
              <Button
                className="ml-auto"
                disabled={!canManage || busy}
                type="text"
                onClick={() => onReview(version.id)}
              >
                {installed ? t(($) => $.plugins.publish.review_upgrade) : t(($) => $.plugins.publish.review_install)}
              </Button>
            )}
          </li>
        ))}
      </ul>
    </div>
  );
}

function InstalledPlugin({
  installation,
  wsId,
  canManage,
}: {
  installation: PluginInstallation;
  wsId: string;
  canManage: boolean;
}) {
  const { t } = useT("settings");
  const enabledMutation = useSetPluginEnabled(wsId);
  const uninstallMutation = useUninstallPlugin(wsId);
  const isMutating = enabledMutation.isPending || uninstallMutation.isPending;

  const reportError = (error: unknown) => {
    toast.error(error instanceof Error ? error.message : t(($) => $.plugins.action_failed));
  };

  // Only rendered when a hook contributes one: a plugin with no hooks has no
  // failing calls to report.
  const hasHooks = (installation.hooks ?? []).length > 0;

  // An mcp hook is inert until its tools are approved, so the panel appears for
  // every one of them rather than only when something is already pinned.
  const mcpContributions = mcpHooks(installation.hooks ?? []);
  const scheduledHooks = (installation.hooks ?? []).filter((hook) => hook.schedule !== undefined);

  const contributions = [
    ...installation.surfaces.map((surface) => `${surface.name} (${surface.type})`),
    ...installation.hooks.map((hook) => `${hook.name} (${hook.triggers.join(", ")})`),
    ...installation.resources.map((resource) => `${resource.key} (${resource.type})`),
  ];

  // The card became the group, and its two `divide-y` children are the group's
  // body: the identity block moved into the header (`title` / `description` /
  // `extra`) and the configuration form keeps its top border as the separator.
  //
  // A group with no `title` still renders its header, so the plugin's name has
  // to live there rather than in the body — the alternative is a dead band
  // inside the panel. The switch and the uninstall pill go to `extra`, which
  // renders on both of `FormGroup`'s trees, unlike `desc`.
  const versionBadge = <Badge variant="secondary">{t(($) => $.plugins.version, { version: installation.version })}</Badge>;
  const disabledBadge = <Badge variant="outline">{t(($) => $.plugins.states.disabled)}</Badge>;

  return (
    <SettingsGroup
      description={<span className="text-caption">{installation.plugin_key}</span>}
      extra={
        <div className="flex items-center gap-2">
          {/* Lobe's `Switch` component destructures a fixed prop list with no
              rest spread, so it drops `aria-label` and `onCheckedChange` and
              the toggle freezes with no error. `SettingsSwitch` is built on the
              atom that spreads both. */}
          <SettingsSwitch
            checked={installation.enabled}
            disabled={!canManage || isMutating}
            label={t(($) => $.plugins.enabled_label)}
            onCheckedChange={(checked) => enabledMutation
              .mutateAsync({ installationId: installation.id, enabled: checked === true })
              .then(() => toast.success(checked
                ? t(($) => $.plugins.enabled)
                : t(($) => $.plugins.disabled)))
              .catch(reportError)}
          />
          {/* `SettingsPillButton tone="destructive"` was `type="fill"` +
              `danger` on a transparent border, not a solid primary-danger. */}
          <Button
            aria-label={t(($) => $.plugins.uninstall)}
            danger
            disabled={!canManage || isMutating}
            loading={uninstallMutation.isPending}
            shape="round"
            type="fill"
            onClick={() => uninstallMutation.mutateAsync(installation.id)
              .then(() => toast.success(t(($) => $.plugins.uninstalled)))
              .catch(reportError)}
          >
            {t(($) => $.plugins.uninstall)}
          </Button>
        </div>
      }
      title={
        <span className="inline-flex flex-wrap items-center gap-2">
          {installation.name}
          {versionBadge}
          {installation.enabled ? null : disabledBadge}
        </span>
      }
    >
      <div className="space-y-4 py-4">
        {hasHooks ? <PluginHookActivity wsId={wsId} installationId={installation.id} /> : null}

        {installation.description ? (
          <p className="max-w-2xl text-caption">{installation.description}</p>
        ) : null}

        <div className="space-y-2">
          <div className="text-caption font-medium">{t(($) => $.plugins.granted_scopes)}</div>
          <ScopeList scopes={installation.granted_scopes} />
        </div>

        {scheduledHooks.length > 0 ? (
          <div className="space-y-1">
            <div className="flex items-center gap-2 text-caption font-medium">
              <CalendarClock className="size-4" />
              {t(($) => $.plugins.schedule.title)}
            </div>
            <ScheduleList hooks={scheduledHooks} showNextRun />
            <PluginScheduleActivity
              wsId={wsId}
              installationId={installation.id}
              hooks={scheduledHooks}
            />
          </div>
        ) : null}

        {mcpContributions.length > 0 ? (
          <div className="space-y-2">
            <div className="text-caption font-medium">{t(($) => $.plugins.mcp.badge)}</div>
            {mcpContributions.map((hook) => (
              <PluginMCPApproval
                key={hook.key}
                wsId={wsId}
                installationId={installation.id}
                hook={hook}
                canManage={canManage}
              />
            ))}
          </div>
        ) : null}

        {contributions.length > 0 ? (
          <div className="space-y-1">
            <div className="text-caption font-medium">{t(($) => $.plugins.contributes)}</div>
            <p className="text-caption text-muted-foreground">{contributions.join(" · ")}</p>
          </div>
        ) : null}
      </div>

      <ConfigForm installation={installation} canManage={canManage} wsId={wsId} />
    </SettingsGroup>
  );
}

/**
 * The workspace's plugin screen.
 *
 * **`SettingsTab` is gone, and with it the page heading.** Inside the settings
 * dialog it returned `children` and nothing else (`settings-layout.tsx`), so its
 * `title`, `description` and `action` were all dropped there. The title belongs
 * to `settings-page.tsx`'s `DialogHeader`, which already renders
 * `page.tabs.plugins` — and `plugins.title` resolves to the *same* string
 * ("Plugins" / "Plugins"), so it must not come back as a group title. The
 * description does not have that home — `tabDescription("plugins")` returns
 * `""` — so it stays with the panel as the lede above the groups, the same
 * placement `billing-tab` gave its own. Its proper home is `tabDescription`,
 * which owns this copy for the tabs that already have it.
 *
 * **`action`: this tab had none, and that is a measurement.** `SettingsTab` was
 * passed only `title` and `description`. The add/publish controls are group
 * content, not a page-level action.
 *
 * **The root container is `space-y-8`.** `SettingsTab`'s nested branch wrapped
 * its children in `space-y-12`, and the tab body sets `gap: 0`, so dropping the
 * wrapper would leave the lede, the read-only notice, the publish group and the
 * installed group with no separation at all. `space-y-8` is the family value
 * (billing, github, repositories, lark, wecom, telegram, slack, weixin,
 * dingtalk, integrations).
 */
export function PluginsTab() {
  const { t } = useT("settings");
  const workspace = useCurrentWorkspace();
  const wsId = workspace?.id ?? "";
  const { role } = useCurrentMember(wsId);
  const canManage = role === "owner" || role === "admin";

  const { data, isLoading, isError } = useQuery(pluginInstallationsOptions(wsId));
  const installations = useMemo(() => data?.plugins ?? [], [data]);

  return (
    <div className="space-y-8">
      <p className="text-body text-muted-foreground">{t(($) => $.plugins.description)}</p>

      {!canManage ? (
        <Alert
          description={t(($) => $.plugins.read_only_description)}
          icon={AlertCircle}
          title={t(($) => $.plugins.read_only)}
          type="secondary"
        />
      ) : null}

      {canManage ? <PublishAndInstall wsId={wsId} canManage={canManage} /> : null}

      {/* `SettingsSection` without a `SettingsCard` is a *bare* section, and a
          bare `Form.Group` is `borderless` — Lobe's own default. The outlined
          variant here would put every plugin's own outlined panel inside a
          second bordered one. */}
      <SettingsGroup title={t(($) => $.plugins.installed.title)} variant="borderless">
        {isLoading ? (
          <Skeleton height={96} aria-label={t(($) => $.plugins.loading)} />
        ) : isError ? (
          <Alert
            description={t(($) => $.plugins.load_failed_description)}
            icon={AlertCircle}
            title={t(($) => $.plugins.load_failed)}
            type="error"
          />
        ) : installations.length === 0 ? (
          <SettingsEmptyState title={t(($) => $.plugins.empty)} />
        ) : (
          <div className="space-y-4">
            {installations.map((installation) => (
              <InstalledPlugin
                key={installation.id}
                installation={installation}
                wsId={wsId}
                canManage={canManage}
              />
            ))}
          </div>
        )}
      </SettingsGroup>
    </div>
  );
}
