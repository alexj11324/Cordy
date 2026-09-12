"use client";

import { useEffect, useMemo, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { AlertTriangle, Check, Loader2, Plug, RefreshCw, Trash2 } from "lucide-react";
// Two design systems in one file, and here the split is by *element kind*
// rather than by surface — `ComposioTab` is rendered from `./integrations-tab`'s
// `content`, which **both** branches of its `standalone ? … : …` ternary render,
// so its host set is exactly that component's two importers:
//
//   settings-page.tsx        — inside <LobeThemeBridge> at its root
//   integrations/index.tsx   — WorkspaceIntegrationsPage, the Web
//                              `/integrations` route; bridged since b8a78232,
//                              and NOT before it
//
// Both are bridged, so nothing here is gated by a host. That was established by
// resolving the **import specifier**, not the identifier: `grep
// "<IntegrationsTab"` also matches `agents/components/tabs/integrations-tab.tsx`,
// a different component sharing the name, which renders none of this family.
// What stays on shadcn is the app
// tile's `Card`: Lobe has no `Card` at all (verified against the installed
// `@lobehub/ui@5.42.6`), and a tile is not one of the four things the family's
// `Card` → `Form.Group` mapping covers — it is not a box around a list of rows,
// not a hand-written section heading, not a placeholder, and its body is not
// prose. It is the item of a 1/2/3-column catalog, and flattening that grid
// into single-column rows would be a design change rather than a migration
// step. The search box, the three placeholders and the confirm dialog below are
// all Lobe.
import { Button as LobeButton } from "@lobehub/ui/base-ui";
import { Card, CardContent } from "@orvilo/ui/components/ui/card";
import { api } from "@orvilo/core/api";
import {
  composioConnectionsOptions,
  composioKeys,
  composioToolkitsOptions,
} from "@orvilo/core/composio";
import type { ComposioToolkit } from "@orvilo/core/types";
import { ComposioToolkitLogo } from "../../common/composio-toolkit-logo";
import { useT, useTimeAgo } from "../../i18n";
import { useNavigation } from "../../navigation";
import { useSettingsConfirm } from "./settings-confirm";
import { SettingsEmptyState } from "./settings-empty";
import { SettingsSearchBar } from "./settings-search";

// ComposioTab renders the connectable Composio toolkit catalog and lets the
// user connect / disconnect the apps their agents can act on.
//
// Key UX rule (MUL-4009): the backend only returns toolkits with an enabled
// auth config in the Composio project, so every card here is connectable —
// toolkits with no auth config are filtered out server-side rather than shown
// with a dead "not configured" hint. The `toolkit.connectable` guard on the
// Connect button is kept as a client-side backstop (older/misbehaving servers
// could still send a non-connectable entry); such an entry simply renders no
// action affordance rather than a broken Connect button that would 400.
export function ComposioTab() {
  const { t } = useT("settings");
  const qc = useQueryClient();
  const navigation = useNavigation();

  const toolkitsQuery = useQuery(composioToolkitsOptions());
  const connectionsQuery = useQuery(composioConnectionsOptions());

  const [query, setQuery] = useState("");
  const [connectingSlug, setConnectingSlug] = useState<string | null>(null);
  // The `disconnectTarget` / `disconnecting` pair of `useState`s is gone with
  // the `AlertDialog`: the imperative confirm owns both the open state and the
  // in-flight spinner, so the row hands its connection id straight through.
  const confirm = useSettingsConfirm();

  // The hosted Composio consent flow is a full-page redirect that lands back
  // on the settings page carrying either `?connected=<slug>` (success) or
  // `?error=composio_connect_failed` (any backend-side failure — see
  // Service.CallbackRedirect, MUL-3720). Consume it exactly once: fire a toast,
  // refresh the connections list so the freshly-linked card flips to Connected
  // without a manual reload, then strip the one-shot params via `replace` so a
  // browser refresh doesn't re-toast.
  const connectedParam = navigation.searchParams.get("connected");
  const errorParam = navigation.searchParams.get("error");
  // React Strict Mode (dev / Next) double-invokes mount effects as
  // mount → cleanup → mount. On the second invoke the `replace` from the first
  // hasn't committed yet, so the closure still sees the same params and would
  // toast + invalidate twice. Guard with a ref keyed on the callback we already
  // consumed; a genuinely new callback (different slug, or the redirect being a
  // full page load that resets this ref) still fires.
  const consumedCallbackKey = useRef<string | null>(null);
  useEffect(() => {
    const callbackKey = connectedParam
      ? `connected:${connectedParam}`
      : errorParam === "composio_connect_failed"
        ? "error:composio_connect_failed"
        : null;
    if (!callbackKey) return;
    if (consumedCallbackKey.current === callbackKey) return;
    consumedCallbackKey.current = callbackKey;
    if (connectedParam) {
      toast.success(t(($) => $.composio.toast_connected));
      void qc.invalidateQueries({ queryKey: composioKeys.connections() });
    } else {
      toast.error(t(($) => $.composio.toast_connect_failed));
    }
    // Drop only the Composio one-shot params; keep everything else (notably
    // ?tab=integrations) so the user stays on this tab.
    const params = new URLSearchParams(navigation.searchParams);
    params.delete("connected");
    params.delete("error");
    const qs = params.toString();
    navigation.replace(qs ? `${navigation.pathname}?${qs}` : navigation.pathname);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [connectedParam, errorParam]);

  // Map active connections by toolkit slug so each card knows whether it is
  // already connected (and which connection id to disconnect).
  const connectionBySlug = useMemo(() => {
    const m = new Map<string, string>();
    for (const c of connectionsQuery.data ?? []) {
      if (c.status === "active") m.set(c.toolkit_slug, c.id);
    }
    return m;
  }, [connectionsQuery.data]);

  // Toolkits whose latest connection is expired render a Reconnect affordance
  // instead of Connected/Connect. Backend only emits `expired` once Stage 4
  // (MUL-3719) lands, but the branch is wired up now so it lights up for free.
  const expiredBySlug = useMemo(() => {
    const m = new Set<string>();
    for (const c of connectionsQuery.data ?? []) {
      if (c.status === "expired") m.add(c.toolkit_slug);
    }
    return m;
  }, [connectionsQuery.data]);

  // Last-used timestamp per active connection, for the "Last used …" line on a
  // connected card. Backend leaves this null until tool-call dispatch starts
  // stamping it (Stage 3, MUL-3721); the card shows a "never used" placeholder
  // until then.
  const lastUsedBySlug = useMemo(() => {
    const m = new Map<string, string | null>();
    for (const c of connectionsQuery.data ?? []) {
      if (c.status === "active") m.set(c.toolkit_slug, c.last_used_at ?? null);
    }
    return m;
  }, [connectionsQuery.data]);

  const toolkits = useMemo(() => toolkitsQuery.data ?? [], [toolkitsQuery.data]);
  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return toolkits;
    return toolkits.filter(
      (tk) =>
        tk.name.toLowerCase().includes(q) ||
        tk.slug.toLowerCase().includes(q) ||
        (tk.category ?? "").toLowerCase().includes(q),
    );
  }, [toolkits, query]);

  // 503 handling lives in the parent IntegrationsTab, which hides the whole
  // Composio section when COMPOSIO_API_KEY is unset — this component only
  // mounts when the integration is configured, so it deals with the loaded /
  // error / empty / list states below.

  async function handleConnect(tk: ComposioToolkit) {
    if (connectingSlug) return;
    setConnectingSlug(tk.slug);
    try {
      const { redirect_url } = await api.beginComposioConnect(tk.slug);
      // Hand the browser to Composio's hosted consent flow; it redirects back
      // to /api/integrations/composio/callback when done.
      window.location.href = redirect_url;
    } catch (e) {
      toast.error(e instanceof Error ? e.message : t(($) => $.composio.connect_failed));
      setConnectingSlug(null);
    }
  }

  /**
   * Rejects on failure on purpose: `useSettingsConfirm`'s `onOk` keeps the
   * dialog open only while its promise is unsettled, so swallowing the error
   * here would dismiss the confirmation exactly when the connection is still
   * live — the optimistic-removal shape the project's state rules forbid on
   * destructive flows. The toast is what the user reads; the rethrow is what
   * holds the dialog.
   */
  async function handleDisconnect(connectionId: string) {
    try {
      await api.deleteComposioConnection(connectionId);
      await qc.invalidateQueries({ queryKey: composioKeys.connections() });
      toast.success(t(($) => $.composio.toast_disconnected));
    } catch (e) {
      toast.error(e instanceof Error ? e.message : t(($) => $.composio.disconnect_failed));
      throw e;
    }
  }

  const openDisconnectConfirm = (connectionId: string) =>
    confirm({
      title: t(($) => $.composio.disconnect_confirm_title),
      description: t(($) => $.composio.disconnect_confirm_description),
      confirmLabel: t(($) => $.composio.disconnect),
      cancelLabel: t(($) => $.composio.disconnect_confirm_cancel),
      onConfirm: () => handleDisconnect(connectionId),
    });

  return (
    <div className="space-y-6">
      <section className="space-y-1">
        <p className="text-body text-muted-foreground">{t(($) => $.composio.page_description)}</p>
      </section>

      {toolkitsQuery.isLoading ? (
        <SettingsEmptyState title={t(($) => $.composio.loading)} />
      ) : toolkitsQuery.isError ? (
        // Same placeholder as an empty catalog — there is no group to wrap it in
        // here (the catalogue is not a section of rows), and a `Form.Group` with
        // no children would draw a header over an empty padded panel — but with
        // `tone="danger"`, because the base component rendered this sentence in
        // `text-destructive` and a failed fetch must not read as "you have no
        // apps". The card chrome is what changed; the tone is not ours to drop.
        //
        // This is NOT the Telegram tab's case, and an earlier version of this
        // comment said it was. Telegram's load failure was already
        // `text-muted-foreground`, so folding it into a neutral placeholder
        // preserved its tone; composio's was `text-destructive`, so the same
        // action lost it. Same shape, opposite effect — check the tone of the
        // text you are folding, not whether a neighbour folds.
        <SettingsEmptyState
          title={t(($) => $.composio.load_failed)}
          tone="danger"
        />
      ) : toolkits.length === 0 ? (
        <SettingsEmptyState
          title={t(($) => $.composio.empty_title)}
          description={t(($) => $.composio.empty_description)}
        />
      ) : (
        <section className="space-y-3">
          <SettingsSearchBar
            className="max-w-xs"
            // `label` is the accessible name and the placeholder is not: the
            // old `Input` had no label at all, so the catalog search box had
            // none either. Same string for both, the way `labels-tab` does it.
            label={t(($) => $.composio.search_placeholder)}
            placeholder={t(($) => $.composio.search_placeholder)}
            value={query}
            onValueChange={setQuery}
          />
          {connectionsQuery.isError && (
            // Don't silently treat a failed connections fetch as "nothing
            // connected" — that would hide real connections and offer Connect
            // on something already linked. Surface it so the user knows the
            // connected state may be incomplete; the catalog still renders.
            <p className="text-caption text-destructive">
              {t(($) => $.composio.connections_load_failed)}
            </p>
          )}
          <div className="grid grid-cols-1 gap-2 sm:grid-cols-2 lg:grid-cols-3">
            {filtered.map((tk) => (
              <ToolkitCard
                key={tk.slug}
                toolkit={tk}
                connectionId={connectionBySlug.get(tk.slug)}
                expired={expiredBySlug.has(tk.slug)}
                lastUsedAt={lastUsedBySlug.get(tk.slug) ?? null}
                connecting={connectingSlug === tk.slug}
                anyConnecting={connectingSlug !== null}
                onConnect={() => handleConnect(tk)}
                onDisconnect={openDisconnectConfirm}
              />
            ))}
          </div>
        </section>
      )}
    </div>
  );
}

function ToolkitCard({
  toolkit,
  connectionId,
  expired,
  lastUsedAt,
  connecting,
  anyConnecting,
  onConnect,
  onDisconnect,
}: {
  toolkit: ComposioToolkit;
  connectionId?: string;
  expired: boolean;
  lastUsedAt: string | null;
  connecting: boolean;
  anyConnecting: boolean;
  onConnect: () => void;
  onDisconnect: (connectionId: string) => void;
}) {
  const { t } = useT("settings");
  const timeAgo = useTimeAgo();
  const isConnected = !!connectionId;

  return (
    <Card>
      <CardContent className="flex items-center gap-3 p-3">
        <ComposioToolkitLogo
          slug={toolkit.slug}
          name={toolkit.name}
          fallbackLogo={toolkit.logo}
        />
        <div className="min-w-0 flex-1">
          <p className="truncate text-body font-medium">{toolkit.name || toolkit.slug}</p>
          {isConnected ? (
            // Last-used line. Backend leaves last_used_at null until Stage 3
            // dispatch stamps it, so show a localized "never used" placeholder
            // rather than hiding the line entirely.
            <p className="truncate text-micro text-muted-foreground">
              {lastUsedAt
                ? t(($) => $.composio.last_used, { when: timeAgo(lastUsedAt) })
                : t(($) => $.composio.last_used_never)}
            </p>
          ) : toolkit.category ? (
            <p className="truncate text-micro uppercase tracking-wide text-muted-foreground">
              {toolkit.category}
            </p>
          ) : null}
        </div>

        {isConnected ? (
          <div className="flex items-center gap-2">
            <span className="inline-flex items-center gap-1 text-caption text-emerald-600">
              <Check className="h-3 w-3" />
              {t(($) => $.composio.connected)}
            </span>
            <LobeButton
              aria-label={t(($) => $.composio.disconnect)}
              icon={<Trash2 className="h-3 w-3" />}
              onClick={() => onDisconnect(connectionId!)}
            />
          </div>
        ) : expired ? (
          // Token-expired connection: surface the failure and let the user
          // re-run the same connect flow in one click (no disconnect step).
          <div className="flex items-center gap-2">
            <span className="inline-flex items-center gap-1 text-caption text-amber-600">
              <AlertTriangle className="h-3 w-3" />
              {t(($) => $.composio.expired)}
            </span>
            <LobeButton disabled={anyConnecting} onClick={onConnect}>
              {connecting ? (
                <Loader2 className="h-3 w-3 animate-spin" />
              ) : (
                <RefreshCw className="h-3 w-3" />
              )}
              {connecting ? t(($) => $.composio.connecting) : t(($) => $.composio.reconnect)}
            </LobeButton>
          </div>
        ) : toolkit.connectable ? (
          <LobeButton disabled={anyConnecting} type="primary" onClick={onConnect}>
            {connecting ? (
              <Loader2 className="h-3 w-3 animate-spin" />
            ) : (
              <Plug className="h-3 w-3" />
            )}
            {connecting ? t(($) => $.composio.connecting) : t(($) => $.composio.connect)}
          </LobeButton>
        ) : null}
      </CardContent>
    </Card>
  );
}
