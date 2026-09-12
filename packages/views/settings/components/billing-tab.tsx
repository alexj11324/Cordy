"use client";

import { useEffect, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import {
  AlertCircle,
  CheckCircle2,
  CreditCard,
  ExternalLink,
  Loader2,
  Plus,
  RefreshCw,
} from "lucide-react";
import { ApiError, errorCode } from "@orvilo/core/api";
import { automationQuotaUsageOptions } from "@orvilo/core/automations";
import {
  useCreateWorkspaceSubscriptionCheckout,
  useCreateWorkspaceSubscriptionPortal,
  usePreviewWorkspaceSeatPurchase,
  usePurchaseWorkspaceSeats,
  issueLimitUsageOptions,
  workspaceSubscriptionPricesOptions,
  workspaceSubscriptionSummaryOptions,
} from "@orvilo/core/billing";
import { useFeatureEnabled } from "@orvilo/core/config";
import { BILLING_WORKSPACE_SUBSCRIPTIONS_FLAG } from "@orvilo/core/feature-flags";
import { useCurrentWorkspace } from "@orvilo/core/paths";
import type {
  PurchaseWorkspaceSeatsRequest,
  WorkspaceSeatPurchasePreview,
  WorkspaceSubscriptionInterval,
} from "@orvilo/core/types";
import { Alert, Button, Input, Modal, Skeleton } from "@lobehub/ui/base-ui";
import { Badge } from "@orvilo/ui/components/ui/badge";
import {
  Progress,
  ProgressLabel,
  ProgressValue,
} from "@orvilo/ui/components/ui/progress";
import { useLocale, useT } from "../../i18n";
import { useNavigation } from "../../navigation";
import { openExternal } from "../../platform";
import { SettingsFormRow, SettingsGroup } from "./settings-shell";
import {
  hasActiveWorkspaceSeatCapacity,
  resolveAutomationUsage,
} from "./billing-state";
import { formatStripeMinorAmount } from "./billing-format";

export { formatStripeMinorAmount } from "./billing-format";

const CHECKOUT_SYNC_TIMEOUT_MS = 30_000;
const SEAT_PURCHASE_POLL_TIMEOUT_MS = 2 * 60_000;
const SEAT_PURCHASE_PREVIEW_DEBOUNCE_MS = 800;

type WorkspaceBillingReturnResult = "success" | "cancel" | "portal";

function parseReturnResult(
  value: string | null,
): WorkspaceBillingReturnResult | null {
  switch (value) {
    case "success":
    case "cancel":
    case "portal":
      return value;
    default:
      return null;
  }
}

function createIdempotencyKey(prefix: string, wsId: string): string {
  const suffix =
    globalThis.crypto?.randomUUID?.() ??
    `${Date.now()}-${Math.random().toString(36).slice(2)}`;
  return `${prefix}-${wsId}-${suffix}`.slice(0, 255);
}

function formatDate(value: string | null, locale: string): string | null {
  if (!value) return null;
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return null;
  return new Intl.DateTimeFormat(locale, { dateStyle: "medium" }).format(date);
}

function formatDateTime(value: string | null, locale: string): string | null {
  if (!value) return null;
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return null;
  return new Intl.DateTimeFormat(locale, {
    dateStyle: "medium",
    timeStyle: "short",
  }).format(date);
}

function planBadgeVariant(plan: string): "default" | "secondary" | "outline" {
  if (plan === "pro") return "default";
  if (plan === "free") return "secondary";
  return "outline";
}

function statusBadgeVariant(
  status: string,
): "default" | "secondary" | "destructive" | "outline" {
  switch (status) {
    case "active":
    case "trialing":
      return "default";
    case "past_due":
    case "incomplete":
    case "unpaid":
      return "destructive";
    case "inactive":
    case "canceled":
    case "incomplete_expired":
    case "paused":
      return "secondary";
    default:
      return "outline";
  }
}

/**
 * Workspace billing — plan, limits, seats, and the two Stripe flows.
 *
 * **`SettingsTab` is gone, and with it the page heading.** It returned
 * `children` and nothing else inside the settings dialog
 * (`settings-layout.tsx`), so its `title` and `description` were already
 * dropped on this tab's only surface. The title belongs to `settings-page.tsx`'s
 * `DialogHeader`; the description does not — `tabDescription("billing")`
 * returns `""`, so the header renders the title alone and this sentence would
 * have been dropped rather than relocated. It stays with the panel, above
 * whatever the panel turns out to render — the groups, the loading placeholder,
 * or the failure alert — because it describes the panel rather than any one
 * group. (Its proper home is `tabDescription`, which owns that copy for the
 * tabs whose description is already there; that file is not this one.)
 *
 * **The group titles must not be `workspace.title`.** That key resolves to
 * "Billing" / "账单与套餐" in both locales — the identical string
 * `page.tabs.billing` puts in the `DialogTitle` above — so a group carrying it
 * would print the page title a second time. That is the defect Task 6 shipped
 * in `labels-tab` and had to fix; the check is on the two i18n keys, not on how
 * the two sentences read. The five group titles here (`current`, `upgrade`,
 * `management`, `limits`, `seats`) are all section names, and the two
 * single-state branches use `workspace.loading` / `workspace.load_failed.title`
 * for the same reason.
 *
 * **`action`: this tab had none, and that is a measurement.**
 * `billing-tab.tsx` passed only `title` and `description` to `SettingsTab`, so
 * there was no page-level control for the nested branch to swallow. (The two
 * this file does own render in the `extra`-less body: the upgrade CTA and the
 * seat-purchase CTA are group content, not a section `action`.)
 *
 * **The Checkout confirmation is a controlled `Modal`, not
 * `useSettingsConfirm`.** Its dismissal has a side effect on *every* exit the
 * user can take: the idempotency intent is released, so an attempt the user
 * backed out of is not replayed by the next one. (The programmatic close after
 * a Checkout outcome is the deliberate exception — an ambiguous failure keeps
 * the key so the retry replays the same Session.) `confirmModal` cannot see
 * every exit — its `onCancel`, and the wrapper's, fire for the Cancel button
 * alone, and Escape goes through `StackItem`'s `handleOpenChange`, which never
 * reads that config — so routing this dialog through the shared channel
 * narrowed a four-path release to one and made the next Upgrade replay the
 * abandoned Stripe Session. See `handleCheckoutConfirmOpenChange`. The
 * seat-purchase dialog above is also a `Modal`, for the ordinary reason: it
 * collects input.
 *
 * **Every branch is `space-y-8`.** `SettingsTab`'s nested branch wrapped the
 * children it was given in `space-y-12`, and dropping `SettingsTab` left the
 * lede, the banners and the five groups with no separation at all — the tab
 * body sets `gap: 0` and neither Lobe's `Alert` nor `Form.Group` carries a
 * margin of its own. `space-y-8` is what every other migrated tab uses (lark,
 * wecom, telegram, slack, weixin, dingtalk, integrations), so the four tabs that
 * lost this spacing are restored to that value rather than to a new one.
 *
 * **Four buttons are `size="small"` and stay that way.** F5 asked for the
 * default `middle` on this tab's buttons, and its stated reason was alignment
 * beside a 32px `Select`; none of these four sits next to one. They are icon-led
 * refresh controls (`prices`, automation quota, seat summary, seat purchase)
 * living in an `Alert` action slot or beside `text-caption` copy, where
 * `middle` would outweigh the text they belong to.
 *
 * The loading branch's `workspace.loading` title is **newly visible**: the
 * pre-migration loading state used `SettingsCard`, which had no title, so
 * "Loading workspace billing" / "正在加载工作区账单" now appears where nothing
 * did. It is a different key with a different value from `page.tabs.billing` in
 * both locales, so it is not the page title repeated.
 *
 * `Badge` and `Progress` stay shadcn. `Progress` is decision 7; `Badge` is a
 * sanctioned exemption — the migration reference records that Lobe has no
 * `Badge` and that `Tag` is a different component rather than a rename of it,
 * so swapping it would change what the plan and status pills *mean*, not just
 * how they look.
 *
 * A second gate inside the tab keeps direct/test mounts fail-closed. The
 * Settings shell also omits this component and its navigation entry while the
 * flag is absent, so no subscription request is issued in either path.
 */
export function BillingTab() {
  const enabled = useFeatureEnabled(
    BILLING_WORKSPACE_SUBSCRIPTIONS_FLAG,
    false,
  );
  return enabled ? <BillingTabContent /> : null;
}

function BillingTabContent() {
  const { t } = useT("billing");
  const locale = useLocale();
  const navigation = useNavigation();
  const workspace = useCurrentWorkspace();
  const wsId = workspace?.id ?? "";
  const returnResultParam = parseReturnResult(
    navigation.searchParams.get("result"),
  );
  const returnSessionId = navigation.searchParams.get("session_id");
  const callbackKey =
    navigation.searchParams.has("result") ||
    navigation.searchParams.has("session_id")
      ? `${navigation.searchParams.get("result") ?? ""}:${returnSessionId ?? ""}`
      : null;
  const [returnState, setReturnState] = useState<{
    workspaceId: string | null;
    result: WorkspaceBillingReturnResult | null;
    observedAt: number | null;
  }>(() => ({
    workspaceId: wsId || null,
    result: returnResultParam,
    observedAt: returnResultParam === "success" ? Date.now() : null,
  }));
  const returnStateMatchesWorkspace =
    returnState.workspaceId === null || returnState.workspaceId === wsId;
  const returnResult = returnStateMatchesWorkspace ? returnState.result : null;
  const returnObservedAt = returnStateMatchesWorkspace
    ? returnState.observedAt
    : null;
  const [interval, setInterval] =
    useState<WorkspaceSubscriptionInterval>("month");
  const [seatPurchaseOpen, setSeatPurchaseOpen] = useState(false);
  const [additionalSeatsInput, setAdditionalSeatsInput] = useState("1");
  const [seatPreview, setSeatPreview] =
    useState<WorkspaceSeatPurchasePreview | null>(null);
  const [seatPreviewRevision, setSeatPreviewRevision] = useState(0);
  const [seatPreviewRefreshing, setSeatPreviewRefreshing] = useState(false);
  const [seatPurchaseError, setSeatPurchaseError] = useState<string | null>(
    null,
  );
  const [seatPurchasePollingTimedOut, setSeatPurchasePollingTimedOut] =
    useState(false);
  const [actionError, setActionError] = useState<string | null>(null);
  const [portalUnavailable, setPortalUnavailable] = useState(false);
  const [seatPurchaseMessage, setSeatPurchaseMessage] = useState<string | null>(
    null,
  );
  const [isSyncingCheckout, setIsSyncingCheckout] = useState(
    returnResult === "success",
  );
  const [syncTimedOut, setSyncTimedOut] = useState(false);
  const checkoutIntentRef = useRef<{
    wsId: string;
    interval: WorkspaceSubscriptionInterval;
    key: string;
  } | null>(null);
  const portalIntentKeyRef = useRef<string | null>(null);
  const seatPurchaseIntentRef = useRef<{
    wsId: string;
    key: string;
    preview: WorkspaceSeatPurchasePreview;
    request: PurchaseWorkspaceSeatsRequest;
  } | null>(null);
  const seatPreviewInputRef = useRef("");
  const seatPreviewCapacityRetryRef = useRef<{
    inputKey: string;
    attempts: number;
  } | null>(null);
  const consumedCallbackKeyRef = useRef<string | null>(null);
  const [checkoutConfirmOpen, setCheckoutConfirmOpen] = useState(false);

  useEffect(() => {
    checkoutIntentRef.current = null;
    portalIntentKeyRef.current = null;
    seatPurchaseIntentRef.current = null;
    seatPreviewInputRef.current = "";
    seatPreviewCapacityRetryRef.current = null;
    setSeatPurchaseOpen(false);
    setSeatPreview(null);
    setSeatPreviewRevision(0);
    setSeatPreviewRefreshing(false);
    setSeatPurchaseError(null);
    setSeatPurchasePollingTimedOut(false);
    setPortalUnavailable(false);
    setActionError(null);
    setSeatPurchaseMessage(null);
  }, [wsId]);

  // Consume Stripe callback params once, then remove them with replace so a
  // refresh, copied URL, or settings-tab round trip cannot replay a banner or
  // restart subscription polling. Keep unrelated params, especially `tab`.
  useEffect(() => {
    if (!callbackKey || consumedCallbackKeyRef.current === callbackKey) return;
    consumedCallbackKeyRef.current = callbackKey;
    setReturnState({
      workspaceId: wsId || null,
      result: returnResultParam,
      observedAt: returnResultParam === "success" ? Date.now() : null,
    });
    if (returnResultParam === "cancel") checkoutIntentRef.current = null;

    const params = new URLSearchParams(navigation.searchParams);
    params.delete("result");
    params.delete("session_id");
    const query = params.toString();
    navigation.replace(
      query ? `${navigation.pathname}?${query}` : navigation.pathname,
    );
  }, [callbackKey, navigation, returnResultParam, wsId]);

  useEffect(() => {
    if (returnResult !== "success") {
      setIsSyncingCheckout(false);
      setSyncTimedOut(false);
      return;
    }
    setIsSyncingCheckout(true);
    setSyncTimedOut(false);
    const timeout = window.setTimeout(() => {
      setIsSyncingCheckout(false);
      setSyncTimedOut(true);
    }, CHECKOUT_SYNC_TIMEOUT_MS);
    return () => window.clearTimeout(timeout);
  }, [returnResult, wsId]);

  const summaryQuery = useQuery({
    ...workspaceSubscriptionSummaryOptions(wsId),
    // Checkout activation and seat additions both finish asynchronously in a
    // Stripe webhook. Seat polling is bounded so a lost webhook cannot keep a
    // browser polling forever.
    refetchInterval: (query) =>
      isSyncingCheckout ||
      (query.state.data?.seatCapacity?.activePurchase &&
        !seatPurchasePollingTimedOut)
        ? 2_000
        : false,
  });
  const entitlements = summaryQuery.data?.entitlement;
  const isCheckoutConfirmed =
    returnResult === "success" &&
    returnObservedAt !== null &&
    summaryQuery.data?.availableActions.checkout === false &&
    summaryQuery.isFetchedAfterMount &&
    summaryQuery.dataUpdatedAt >= returnObservedAt;
  const activeSeatPurchaseRequestId =
    summaryQuery.data?.seatCapacity?.activePurchase?.requestId ?? null;

  useEffect(() => {
    if (!activeSeatPurchaseRequestId) {
      setSeatPurchasePollingTimedOut(false);
      return;
    }
    setSeatPurchasePollingTimedOut(false);
    const timeout = window.setTimeout(
      () => setSeatPurchasePollingTimedOut(true),
      SEAT_PURCHASE_POLL_TIMEOUT_MS,
    );
    return () => window.clearTimeout(timeout);
  }, [activeSeatPurchaseRequestId]);
  const summaryUnavailable =
    summaryQuery.isError ||
    (!summaryQuery.isPending && summaryQuery.data == null);
  const quotaUsageQuery = useQuery(automationQuotaUsageOptions(wsId));
  const issueLimitUsageQuery = useQuery({
    ...issueLimitUsageOptions(wsId),
    enabled:
      wsId.length > 0 &&
      entitlements?.limits.issueCount.mode === "limited",
  });
  const hasSeatCapacity = hasActiveWorkspaceSeatCapacity(summaryQuery.data);
  const canUpgrade = summaryQuery.data?.availableActions.checkout === true;
  const pricesQuery = useQuery({
    ...workspaceSubscriptionPricesOptions(wsId),
    enabled: wsId.length > 0 && canUpgrade,
  });
  const checkoutMutation = useCreateWorkspaceSubscriptionCheckout(wsId);
  const portalMutation = useCreateWorkspaceSubscriptionPortal(wsId);
  const previewSeatPurchaseMutation = usePreviewWorkspaceSeatPurchase();
  const purchaseSeatsMutation = usePurchaseWorkspaceSeats(wsId);
  const refetchSummary = summaryQuery.refetch;
  const previewSeatPurchase = previewSeatPurchaseMutation.mutateAsync;

  useEffect(() => {
    if (!seatPurchaseOpen) return;
    const value = additionalSeatsInput.trim();
    const additionalSeats = /^\d+$/.test(value) ? Number(value) : 0;
    const currentSeats = summaryQuery.data?.seatCapacity?.purchased ?? null;
    const purchaseVersion = summaryQuery.data?.seatCapacity?.version ?? null;
    const retryInputKey = `${wsId}:${value}`;
    if (seatPreviewCapacityRetryRef.current?.inputKey !== retryInputKey) {
      seatPreviewCapacityRetryRef.current = {
        inputKey: retryInputKey,
        attempts: 0,
      };
    }
    const requestKey = `${retryInputKey}:${currentSeats ?? ""}:${purchaseVersion ?? ""}:${seatPreviewRevision}`;
    seatPreviewInputRef.current = requestKey;
    const intent = seatPurchaseIntentRef.current;
    if (
      intent?.wsId === wsId &&
      intent.request.additionalSeats === additionalSeats &&
      intent.request.expectedCurrentSeats === currentSeats &&
      intent.request.expectedPurchaseVersion === purchaseVersion
    ) {
      setSeatPreview(intent.preview);
      setSeatPreviewRefreshing(false);
      setSeatPurchaseError(null);
      return;
    }
    seatPurchaseIntentRef.current = null;
    setSeatPreview(null);
    if (
      !Number.isSafeInteger(additionalSeats) ||
      additionalSeats < 1 ||
      currentSeats === null ||
      purchaseVersion === null
    ) {
      if (
        seatPreviewCapacityRetryRef.current?.inputKey === retryInputKey &&
        seatPreviewCapacityRetryRef.current.attempts > 0
      ) {
        setSeatPurchaseError(
          t(($) => $.workspace.seat_purchase.preview_failed),
        );
      }
      setSeatPreviewRefreshing(false);
      return;
    }

    const timeout = window.setTimeout(() => {
      void previewSeatPurchase({ additionalSeats })
        .then((preview) => {
          if (seatPreviewInputRef.current !== requestKey) return;
          if (
            !preview ||
            preview.additionalSeats !== additionalSeats ||
            preview.currentSeats !== currentSeats ||
            preview.purchaseVersion !== purchaseVersion ||
            preview.resultingSeats !== currentSeats + additionalSeats
          ) {
            setSeatPreviewRefreshing(false);
            setSeatPurchaseError(
              t(($) => $.workspace.seat_purchase.preview_unreadable),
            );
            return;
          }
          setSeatPreviewRefreshing(false);
          setSeatPreview(preview);
          setSeatPurchaseError(null);
        })
        .catch(async (error: unknown) => {
          if (seatPreviewInputRef.current !== requestKey) return;
          const previewErrorCode =
            error instanceof ApiError && error.status === 409
              ? errorCode(error)
              : null;
          if (previewErrorCode === "seat_capacity_changed") {
            const retry = seatPreviewCapacityRetryRef.current;
            if (retry?.inputKey !== retryInputKey || retry.attempts >= 1) {
              setSeatPreviewRefreshing(false);
              setSeatPurchaseError(
                t(($) => $.workspace.seat_purchase.capacity_out_of_sync),
              );
              return;
            }
            retry.attempts += 1;
            setSeatPreviewRefreshing(true);
            setSeatPurchaseError(null);
            try {
              const refreshed = await refetchSummary();
              if (seatPreviewInputRef.current !== requestKey) return;
              if (refreshed.isError === true) {
                setSeatPreviewRefreshing(false);
                setSeatPurchaseError(
                  t(($) => $.workspace.seat_purchase.preview_failed),
                );
                return;
              }
              const refreshedSeats =
                refreshed.data?.seatCapacity?.purchased ?? null;
              const refreshedVersion =
                refreshed.data?.seatCapacity?.version ?? null;
              if (
                refreshedSeats === currentSeats &&
                refreshedVersion === purchaseVersion
              ) {
                setSeatPreviewRefreshing(false);
                setSeatPurchaseError(
                  t(($) => $.workspace.seat_purchase.capacity_out_of_sync),
                );
                return;
              }
            } catch {
              if (seatPreviewInputRef.current === requestKey) {
                setSeatPreviewRefreshing(false);
                setSeatPurchaseError(
                  t(($) => $.workspace.seat_purchase.preview_failed),
                );
              }
              return;
            }
            if (seatPreviewInputRef.current !== requestKey) return;
            setSeatPreviewRevision((revision) => revision + 1);
            return;
          }
          setSeatPreviewRefreshing(false);
          setSeatPurchaseError(
            previewErrorCode === "seat_purchase_in_progress"
              ? t(($) => $.workspace.seat_purchase.in_progress)
              : t(($) => $.workspace.seat_purchase.preview_failed),
          );
        });
    }, SEAT_PURCHASE_PREVIEW_DEBOUNCE_MS);
    return () => window.clearTimeout(timeout);
  }, [
    additionalSeatsInput,
    previewSeatPurchase,
    refetchSummary,
    seatPurchaseOpen,
    seatPreviewRevision,
    summaryQuery.data?.seatCapacity?.purchased,
    summaryQuery.data?.seatCapacity?.version,
    t,
    wsId,
  ]);

  useEffect(() => {
    if (isSyncingCheckout && isCheckoutConfirmed) {
      setIsSyncingCheckout(false);
      setSyncTimedOut(false);
      checkoutIntentRef.current = null;
    }
  }, [isCheckoutConfirmed, isSyncingCheckout]);

  const planLabel = (plan: string) => {
    switch (plan) {
      case "free":
        return t(($) => $.workspace.plan.free);
      case "pro":
        return t(($) => $.workspace.plan.pro);
      default:
        return t(($) => $.workspace.plan.unknown);
    }
  };

  const statusLabel = (status: string) => {
    switch (status) {
      case "inactive":
        return t(($) => $.workspace.status.inactive);
      case "active":
        return t(($) => $.workspace.status.active);
      case "trialing":
        return t(($) => $.workspace.status.trialing);
      case "past_due":
        return t(($) => $.workspace.status.past_due);
      case "canceled":
        return t(($) => $.workspace.status.canceled);
      case "incomplete":
        return t(($) => $.workspace.status.incomplete);
      case "incomplete_expired":
        return t(($) => $.workspace.status.incomplete_expired);
      case "paused":
        return t(($) => $.workspace.status.paused);
      case "unpaid":
        return t(($) => $.workspace.status.unpaid);
      default:
        return t(($) => $.workspace.status.unknown);
    }
  };

  const reportActionError = (error: unknown, fallback: string) => {
    if (error instanceof ApiError && error.status === 503) {
      setActionError(t(($) => $.workspace.errors.temporarily_unavailable));
      return;
    }
    if (error instanceof ApiError && error.status === 403) {
      setActionError(t(($) => $.workspace.errors.permission_changed));
      return;
    }
    setActionError(fallback);
  };

  const handleCheckout = async () => {
    setActionError(null);
    const existing = checkoutIntentRef.current;
    const intent =
      existing?.wsId === wsId && existing.interval === interval
        ? existing
        : {
            wsId,
            interval,
            key: createIdempotencyKey("workspace-checkout", wsId),
          };
    checkoutIntentRef.current = intent;
    try {
      const response = await checkoutMutation.mutateAsync({
        interval,
        idempotencyKey: intent.key,
      });
      if (!response?.url) {
        setCheckoutConfirmOpen(false);
        setActionError(t(($) => $.workspace.errors.checkout_response));
        return;
      }
      setCheckoutConfirmOpen(false);
      openExternal(response.url, { webTarget: "same-tab" });
    } catch (error) {
      // Closed programmatically rather than through `onOpenChange`, and the two
      // are not interchangeable: this path **keeps** the idempotency intent, so
      // an ambiguous failure retries the same Stripe Session instead of opening
      // a second one. Only a dismissal the user asked for — Cancel or Escape —
      // releases it. That is what the suite pins in "reuses the Checkout
      // idempotency key after an ambiguous failure" against "starts a new
      // Checkout intent after explicit cancellation".
      setCheckoutConfirmOpen(false);
      if (error instanceof ApiError && error.status === 409) {
        checkoutIntentRef.current = null;
        setActionError(t(($) => $.workspace.errors.already_subscribed));
        await summaryQuery.refetch();
        return;
      }
      reportActionError(error, t(($) => $.workspace.errors.checkout_failed));
    }
  };

  /**
   * The Checkout confirmation is a controlled `Modal`, **not**
   * `useSettingsConfirm`. This is the second answer to that question, and the
   * reason the first one was wrong is worth keeping.
   *
   * The `AlertDialog` this replaces released the idempotency intent on *every*
   * dismissal — `onOpenChange` fires for the Cancel button, Escape, the X and a
   * backdrop press alike — so an attempt the user deliberately abandoned is not
   * replayed by the next one. `confirmModal` cannot express that, and it cannot
   * be extended to: `ModalConfirmConfig` has no `onOpenChange` field, and the
   * three non-button exits go through `StackItem.handleOpenChange`, which closes
   * the stack entry without ever reading `config.onCancel`. The wrapper's
   * `onCancel` — and `confirmModal`'s own — fire for the **Cancel button
   * alone**. Routing this dialog through the shared channel narrowed a
   * four-path release to one: pressing Escape, then Upgrade again, replayed the
   * abandoned Stripe Session.
   *
   * So the one-channel rule loses here, by name rather than silently. A
   * confirmation whose dismissal has a side effect on *every* exit needs a
   * channel that can see every exit; a future call site with an exit-dependent
   * side effect should read this paragraph rather than repeat the mistake.
   * `maskClosable={false}` keeps the outside press inert the way the alert
   * dialog's was and `closable={false}` removes the X, so the two remaining
   * exits — Escape and Cancel — both land in `onCancel`.
   *
   * `handleCheckout` resolves rather than rejecting when Checkout fails. The
   * failure is surfaced by the panel's action-error alert, and the dialog closes
   * so the user retries from the same call to action — the sequence the suite
   * pins in "reuses the Checkout idempotency key after an ambiguous failure".
   * That is what the wrapper's reject-to-stay-open contract exists to prevent,
   * and it is the other reason this dialog does not use the wrapper.
   */
  const handleCheckoutConfirmOpenChange = (open: boolean) => {
    setCheckoutConfirmOpen(open);
    if (!open) checkoutIntentRef.current = null;
  };

  const handlePortal = async () => {
    setActionError(null);
    const key =
      portalIntentKeyRef.current ??
      createIdempotencyKey("workspace-portal", wsId);
    portalIntentKeyRef.current = key;
    try {
      const response = await portalMutation.mutateAsync(key);
      if (!response?.url) {
        setActionError(t(($) => $.workspace.errors.portal_response));
        return;
      }
      openExternal(response.url, { webTarget: "same-tab" });
      // A Portal URL is single-use. A later click is a new intent, while a
      // network failure above deliberately retains the key for safe retry.
      portalIntentKeyRef.current = null;
    } catch (error) {
      if (error instanceof ApiError && error.status === 404) {
        portalIntentKeyRef.current = null;
        setPortalUnavailable(true);
        setActionError(t(($) => $.workspace.errors.portal_unavailable));
        await summaryQuery.refetch();
        return;
      }
      reportActionError(error, t(($) => $.workspace.errors.portal_failed));
    }
  };

  const handleSeatPurchase = async () => {
    const confirmedPreview = seatPreview;
    if (!confirmedPreview) return;
    setSeatPurchaseError(null);
    const request: PurchaseWorkspaceSeatsRequest = {
      additionalSeats: confirmedPreview.additionalSeats,
      expectedCurrentSeats: confirmedPreview.currentSeats,
      expectedPurchaseVersion: confirmedPreview.purchaseVersion,
      acceptedProrationAmount: confirmedPreview.prorationAmount,
      currency: confirmedPreview.currency,
      idempotencyKey: "",
    };
    const existing = seatPurchaseIntentRef.current;
    const key =
      existing?.wsId === wsId &&
      existing.request.additionalSeats === request.additionalSeats &&
      existing.request.expectedCurrentSeats === request.expectedCurrentSeats &&
      existing.request.expectedPurchaseVersion ===
        request.expectedPurchaseVersion &&
      existing.request.acceptedProrationAmount ===
        request.acceptedProrationAmount &&
      existing.request.currency === request.currency
        ? existing.key
        : createIdempotencyKey("workspace-seat-purchase", wsId).slice(0, 200);
    request.idempotencyKey = key;
    seatPurchaseIntentRef.current = {
      wsId,
      key,
      preview: confirmedPreview,
      request,
    };
    try {
      const response = await purchaseSeatsMutation.mutateAsync(request);
      if (
        !response ||
        response.currentSeats !== confirmedPreview.currentSeats ||
        response.additionalSeats !== confirmedPreview.additionalSeats ||
        response.resultingSeats !== confirmedPreview.resultingSeats ||
        response.currency !== confirmedPreview.currency
      ) {
        setSeatPurchaseError(
          t(($) => $.workspace.seat_purchase.purchase_unreadable),
        );
        return;
      }
      seatPurchaseIntentRef.current = null;
      seatPreviewInputRef.current = "";
      setSeatPurchaseOpen(false);
      setSeatPreview(null);
      setSeatPurchaseMessage(
        t(($) => $.workspace.seat_purchase.submitted, {
          count: response.resultingSeats,
        }),
      );
      await summaryQuery.refetch();
    } catch (error) {
      if (error instanceof ApiError && error.status === 409) {
        const purchaseErrorCode = errorCode(error);
        seatPurchaseIntentRef.current = null;
        setSeatPreview(null);
        if (purchaseErrorCode === "seat_purchase_in_progress") {
          setSeatPurchaseError(
            t(($) => $.workspace.seat_purchase.in_progress),
          );
          await summaryQuery.refetch();
          return;
        }
        setSeatPurchaseError(t(($) => $.workspace.seat_purchase.quote_changed));
        await summaryQuery.refetch();
        setSeatPreviewRevision((revision) => revision + 1);
        return;
      }
      if (error instanceof ApiError && error.status === 402) {
        seatPurchaseIntentRef.current = null;
        setSeatPreview(null);
        setSeatPurchaseError(
          t(($) => $.workspace.seat_purchase.payment_failed),
        );
        return;
      }
      if (error instanceof ApiError && error.status === 403) {
        seatPurchaseIntentRef.current = null;
        reportActionError(
          error,
          t(($) => $.workspace.seat_purchase.purchase_failed),
        );
        return;
      }
      setSeatPurchaseError(
        t(($) => $.workspace.seat_purchase.purchase_failed),
      );
    }
  };

  const handleSeatPurchaseOpenChange = (open: boolean) => {
    if (!open && purchaseSeatsMutation.isPending) return;
    setSeatPurchaseOpen(open);
    seatPreviewCapacityRetryRef.current = null;
    setSeatPreviewRefreshing(false);
    if (!open) {
      seatPreviewInputRef.current = "";
      return;
    }
    setSeatPurchaseError(null);
    const intent = seatPurchaseIntentRef.current;
    if (intent?.wsId === wsId) {
      setAdditionalSeatsInput(String(intent.request.additionalSeats));
      setSeatPreview(intent.preview);
      return;
    }
    setAdditionalSeatsInput("1");
    setSeatPreview(null);
  };

  // The panel's own description. `SettingsTab` accepted a `description` prop
  // but never rendered it inside the dialog — its nested branch returns
  // `children` and nothing else — so before this migration the sentence was not
  // on screen at all, on any branch. Moving it inline (see the header note) put
  // it on the panel, and it describes the panel rather than any one group, so
  // all three branches below render it. Restricting it to the loaded one would
  // make the sentence appear and disappear as the summary resolved.
  //
  // Nothing asserted it outside the loaded panel, which is why that restriction
  // cost no test: the renderer pass is what showed the loading and failure
  // states rendering a panel with no description.
  const lede = (
    <p className="text-body text-muted-foreground">
      {t(($) => $.workspace.description)}
    </p>
  );

  if (summaryQuery.isPending) {
    return (
      <div className="space-y-8">
        {lede}
        <SettingsGroup title={t(($) => $.workspace.loading)}>
          <div
            className="space-y-4 motion-reduce:[&_[data-slot=skeleton]]:animate-none"
            role="status"
            aria-label={t(($) => $.workspace.loading)}
          >
            <Skeleton className="h-5 w-40" />
            <Skeleton className="h-4 w-full" />
            <Skeleton className="h-4 w-2/3" />
          </div>
        </SettingsGroup>
      </div>
    );
  }

  if (summaryQuery.isError || !entitlements) {
    // No group and no card: the failure is the whole panel, and a card header
    // would need a title this namespace does not have — `workspace.title` is
    // the page title (see the note above) and `load_failed.title` is already
    // the alert's own.
    return (
      <div className="space-y-8">
        {lede}
        <Alert
          type="error"
          icon={AlertCircle}
          title={t(($) => $.workspace.load_failed.title)}
          description={t(($) => $.workspace.load_failed.description)}
          action={
            <Button
              icon={RefreshCw}
              type="fill"
              onClick={() => void summaryQuery.refetch()}
            >
              {t(($) => $.workspace.actions.retry)}
            </Button>
          }
        />
      </div>
    );
  }

  const summaryPeriodEnd = formatDate(
    summaryQuery.data?.entitlement.currentPeriodEnd ?? null,
    locale,
  );
  const graceUntilValue = summaryQuery.data?.graceUntil ?? null;
  const graceUntil = formatDate(graceUntilValue, locale);
  const actualSeats = summaryQuery.data?.humanMembers ?? entitlements.seats;
  const seatCapacity = summaryQuery.data?.seatCapacity ?? null;
  const usedSeats = seatCapacity?.used ?? actualSeats;
  const billedSeats = seatCapacity?.purchased ?? null;
  const pendingSeatQuantity = seatCapacity?.pendingQuantity ?? null;
  const reservedSeats = seatCapacity?.reserved ?? 0;
  const membersExceedPurchasedSeats = seatCapacity?.overcommitted === true;
  const availableSeats = seatCapacity?.available ?? null;
  const activeSeatPurchase = seatCapacity?.activePurchase ?? null;
  const activeSeatPurchaseExpiry = formatDateTime(
    activeSeatPurchase?.expiresAt ?? null,
    locale,
  );
  const canAddSeats =
    summaryQuery.data?.availableActions.purchaseSeats === true;
  const quotaUsage = resolveAutomationUsage(
    entitlements,
    quotaUsageQuery.data,
    quotaUsageQuery.isError,
  );
  const quotaResetAt =
    quotaUsage.kind === "metered"
      ? formatDateTime(quotaUsage.resetAt, locale)
      : null;
  const numberFormatter = new Intl.NumberFormat(locale);
  const issueCountLimit = entitlements.limits.issueCount;
  const issueLimitUsage = issueLimitUsageQuery.data;
  const issueLimitValue =
    issueCountLimit.mode === "unlimited"
      ? t(($) => $.workspace.limits.unlimited)
      : issueLimitUsage?.limit === issueCountLimit.limit
        ? `${numberFormatter.format(issueLimitUsage.used)} / ${numberFormatter.format(issueLimitUsage.limit)}`
        : numberFormatter.format(issueCountLimit.limit);
  const isMutating =
    checkoutMutation.isPending ||
    portalMutation.isPending ||
    purchaseSeatsMutation.isPending;
  const selectedPrice = pricesQuery.data?.[interval] ?? null;
  const formattedUnitPrice = selectedPrice
    ? formatStripeMinorAmount(
        selectedPrice.unitAmount,
        selectedPrice.currency,
        locale,
      )
    : null;
  const hasDisplayableUnitPrice =
    selectedPrice?.intervalCount === 1 && formattedUnitPrice !== null;
  const canRetryPrice = !pricesQuery.isLoading && selectedPrice === null;
  const formattedSeatProration = seatPreview
    ? formatStripeMinorAmount(
        seatPreview.prorationAmount,
        seatPreview.currency,
        locale,
      )
    : null;
  const formattedNextSeatInvoice = seatPreview
    ? formatStripeMinorAmount(
        seatPreview.nextInvoiceAmount,
        seatPreview.currency,
        locale,
      )
    : null;

  return (
    <div className="space-y-8">
      {lede}

      {returnResult === "cancel" ? (
        <Alert
          type="secondary"
          icon={AlertCircle}
          title={t(($) => $.workspace.return.cancel_title)}
          description={t(($) => $.workspace.return.cancel_description)}
        />
      ) : null}

      {returnResult === "portal" ? (
        <Alert
          type="success"
          icon={CheckCircle2}
          title={t(($) => $.workspace.return.portal_title)}
          description={t(($) => $.workspace.return.portal_description)}
        />
      ) : null}

      {returnResult === "success" ? (
        <Alert
          type={isCheckoutConfirmed ? "success" : "info"}
          icon={isCheckoutConfirmed ? CheckCircle2 : Loader2}
          // `Icon` is a component prop, so the spinner animation cannot ride on
          // the node the way it did when the icon was a child. `classNames.icon`
          // wraps it, and Lobe marks that span `aria-hidden`, so the rotation
          // costs the alert no accessible name.
          classNames={{
            icon: isSyncingCheckout
              ? "animate-spin motion-reduce:animate-none"
              : undefined,
          }}
          title={
            isCheckoutConfirmed
              ? t(($) => $.workspace.return.active_title)
              : t(($) => $.workspace.return.syncing_title)
          }
          description={
            isCheckoutConfirmed
              ? t(($) => $.workspace.return.active_description)
              : syncTimedOut
                ? t(($) => $.workspace.return.timeout_description)
                : t(($) => $.workspace.return.syncing_description)
          }
        />
      ) : null}

      {summaryQuery.data?.cancelAtPeriodEnd ? (
        <Alert
          type="secondary"
          icon={AlertCircle}
          title={t(($) => $.workspace.subscription_notice.canceling_title)}
          description={
            summaryPeriodEnd
              ? t(
                  ($) => $.workspace.subscription_notice.canceling_description,
                  { date: summaryPeriodEnd },
                )
              : t(
                  ($) =>
                    $.workspace.subscription_notice
                      .canceling_description_without_date,
                )
          }
        />
      ) : null}

      {entitlements.status === "past_due" ? (
        <Alert
          type="error"
          icon={AlertCircle}
          title={t(($) => $.workspace.past_due.title)}
          description={
            graceUntil
              ? t(($) => $.workspace.past_due.grace_description, {
                  date: graceUntil,
                })
              : t(($) => $.workspace.past_due.description)
          }
        />
      ) : null}

      {entitlements.status === "incomplete" ? (
        <Alert
          type="error"
          icon={AlertCircle}
          title={t(($) => $.workspace.subscription_notice.incomplete_title)}
          description={t(
            ($) => $.workspace.subscription_notice.incomplete_description,
          )}
        />
      ) : null}

      {entitlements.status === "incomplete_expired" ? (
        <Alert
          type="error"
          icon={AlertCircle}
          title={t(
            ($) => $.workspace.subscription_notice.incomplete_expired_title,
          )}
          description={t(
            ($) =>
              $.workspace.subscription_notice.incomplete_expired_description,
          )}
        />
      ) : null}

      {entitlements.status === "paused" ? (
        <Alert
          type="secondary"
          icon={AlertCircle}
          title={t(($) => $.workspace.subscription_notice.paused_title)}
          description={t(
            ($) => $.workspace.subscription_notice.paused_description,
          )}
        />
      ) : null}

      {entitlements.status === "unpaid" ? (
        <Alert
          type="error"
          icon={AlertCircle}
          title={t(($) => $.workspace.subscription_notice.unpaid_title)}
          description={t(
            ($) => $.workspace.subscription_notice.unpaid_description,
          )}
        />
      ) : null}

      {entitlements.status === "canceled" ? (
        <Alert
          type="secondary"
          icon={AlertCircle}
          title={t(($) => $.workspace.subscription_notice.canceled_title)}
          description={t(
            ($) => $.workspace.subscription_notice.canceled_description,
          )}
        />
      ) : null}

      {actionError ? (
        <Alert
          type="error"
          icon={AlertCircle}
          title={t(($) => $.workspace.errors.action_title)}
          description={actionError}
        />
      ) : null}

      <SettingsGroup title={t(($) => $.workspace.current.title)}>
        <SettingsFormRow
          label={t(($) => $.workspace.current.plan)}
          description={t(($) => $.workspace.current.plan_description)}
        >
          <div className="flex flex-wrap items-center gap-2 sm:justify-end">
            <Badge variant={planBadgeVariant(entitlements.plan)}>
              {planLabel(entitlements.plan)}
            </Badge>
            <Badge variant={statusBadgeVariant(entitlements.status)}>
              {statusLabel(entitlements.status)}
            </Badge>
          </div>
        </SettingsFormRow>
        <SettingsFormRow
          divider
          label={t(($) => $.workspace.current.members)}
          description={t(($) => $.workspace.current.members_description)}
        >
          <span className="tabular-nums">
            {t(($) => $.workspace.current.member_count, {
              count: actualSeats,
            })}
          </span>
        </SettingsFormRow>
        {summaryQuery.data?.billingInterval ? (
          <SettingsFormRow
            divider
            label={t(($) => $.workspace.current.billing_interval)}
            description={t(
              ($) => $.workspace.current.billing_interval_description,
            )}
          >
            <span>
              {summaryQuery.data.billingInterval === "month"
                ? t(($) => $.workspace.upgrade.monthly)
                : t(($) => $.workspace.upgrade.yearly)}
            </span>
          </SettingsFormRow>
        ) : null}
        {summaryPeriodEnd ? (
          <SettingsFormRow
            divider
            label={t(($) => $.workspace.current.period_end)}
            description={t(($) => $.workspace.current.period_end_description)}
          >
            <span className="tabular-nums">{summaryPeriodEnd}</span>
          </SettingsFormRow>
        ) : null}
      </SettingsGroup>

      {canUpgrade ? (
        <SettingsGroup
          title={t(($) => $.workspace.upgrade.title)}
          description={t(($) => $.workspace.upgrade.description)}
        >
          {/* No `p-4 sm:p-5`: `Form.Group`'s body already carries `12px 16px`,
              which is the inset the rows beside it use. */}
          <div className="space-y-5">
            <div
              className="inline-flex w-full rounded-lg border border-surface-border p-1 sm:w-auto"
              role="group"
              aria-label={t(($) => $.workspace.upgrade.interval_label)}
            >
              {(["month", "year"] as const).map((value) => (
                <button
                  key={value}
                  type="button"
                  aria-pressed={interval === value}
                  className="min-h-11 flex-1 rounded-md px-4 text-body font-medium text-muted-foreground transition-[color,background-color,box-shadow] hover:text-foreground focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring aria-pressed:bg-surface-selected aria-pressed:text-surface-selected-foreground aria-pressed:shadow-sm sm:min-w-32"
                  onClick={() => {
                    setInterval(value);
                    checkoutIntentRef.current = null;
                  }}
                >
                  {value === "month"
                    ? t(($) => $.workspace.upgrade.monthly)
                    : t(($) => $.workspace.upgrade.yearly)}
                </button>
              ))}
            </div>
            <div className="space-y-2">
              <p className="text-body font-medium">
                {t(($) => $.workspace.upgrade.pro_for_team, {
                  count: actualSeats,
                })}
              </p>
              {pricesQuery.isLoading ? (
                <div
                  className="space-y-2 motion-reduce:[&_[data-slot=skeleton]]:animate-none"
                  role="status"
                  aria-label={t(($) => $.workspace.upgrade.price_loading)}
                >
                  <Skeleton className="h-5 w-48" />
                  <Skeleton className="h-4 w-64 max-w-full" />
                </div>
              ) : hasDisplayableUnitPrice ? (
                <div className="space-y-1">
                  <p className="text-body font-semibold tabular-nums">
                    {t(($) => $.workspace.upgrade.unit_price, {
                      price: formattedUnitPrice,
                    })}
                  </p>
                </div>
              ) : null}
              <p className="max-w-[65ch] text-caption leading-5 text-muted-foreground">
                {t(($) => $.workspace.upgrade.price_at_checkout)}
              </p>
              {canRetryPrice ? (
                <>
                  {/* `loading` swaps the refresh icon for Lobe's spinner, and
                        `disabled` is still set by hand: a Lobe button that is
                        only `loading` keeps the native `disabled` attribute off
                        and guards the click in JS instead. */}
                  <Button
                    icon={RefreshCw}
                    type="fill"
                    size="small"
                    loading={pricesQuery.isFetching}
                    disabled={pricesQuery.isFetching}
                    onClick={() => void pricesQuery.refetch()}
                  >
                    {t(($) => $.workspace.actions.retry)}
                  </Button>
                  {pricesQuery.isFetching ? (
                    <span
                      className="sr-only"
                      role="status"
                      aria-label={t(($) => $.workspace.upgrade.price_loading)}
                    >
                      {t(($) => $.workspace.upgrade.price_loading)}
                    </span>
                  ) : null}
                </>
              ) : null}
            </div>
            <Button
              className="w-full sm:w-auto"
              size="large"
              type="primary"
              icon={CreditCard}
              disabled={isMutating}
              onClick={() => handleCheckoutConfirmOpenChange(true)}
            >
              {t(($) => $.workspace.actions.upgrade)}
            </Button>
          </div>
        </SettingsGroup>
      ) : null}

      {summaryQuery.data?.availableActions.portal === true ? (
        <SettingsGroup
          title={t(($) => $.workspace.management.title)}
          description={t(($) => $.workspace.management.description)}
        >
          <SettingsFormRow
            label={t(($) => $.workspace.management.portal)}
            description={
              portalUnavailable
                ? t(($) => $.workspace.management.portal_unavailable)
                : t(($) => $.workspace.management.portal_description)
            }
          >
            {!portalUnavailable ? (
              <Button
                className="w-full sm:w-auto"
                size="large"
                type="primary"
                icon={ExternalLink}
                loading={portalMutation.isPending}
                disabled={isMutating}
                onClick={() => void handlePortal()}
              >
                {t(($) => $.workspace.actions.manage)}
              </Button>
            ) : null}
          </SettingsFormRow>
        </SettingsGroup>
      ) : null}

      <SettingsGroup
        title={t(($) => $.workspace.limits.title)}
        description={t(($) => $.workspace.limits.description)}
      >
        <SettingsFormRow
          label={t(($) => $.workspace.limits.issues)}
          description={t(($) => $.workspace.limits.issues_description)}
        >
          <span className="tabular-nums">{issueLimitValue}</span>
        </SettingsFormRow>
        <SettingsFormRow
          divider
          label={t(($) => $.workspace.limits.automations)}
          description={t(($) => $.workspace.limits.automations_description)}
        >
          {quotaUsage.kind === "unlimited" ? (
            <span className="tabular-nums">
              {t(($) => $.workspace.limits.unlimited)}
            </span>
          ) : quotaUsageQuery.isPending ? (
            <div
              className="w-full max-w-72 space-y-2 motion-reduce:[&_[data-slot=skeleton]]:animate-none"
              role="status"
              aria-label={t(($) => $.workspace.limits.usage_loading)}
            >
              <Skeleton className="h-5 w-full" />
              <Skeleton className="h-4 w-2/3" />
            </div>
          ) : quotaUsage.kind === "metered" ? (
            <div className="w-full max-w-72 space-y-2">
              <Progress
                value={quotaUsage.progress}
                aria-label={t(($) => $.workspace.limits.usage_label)}
              >
                <ProgressLabel>
                  {quotaUsage.reached
                    ? t(($) => $.workspace.limits.reached)
                    : t(($) => $.workspace.limits.current_usage)}
                </ProgressLabel>
                <ProgressValue>
                  {() =>
                    t(($) => $.workspace.limits.usage_total, {
                      total: numberFormatter.format(quotaUsage.total),
                      limit: numberFormatter.format(quotaUsage.limit),
                    })
                  }
                </ProgressValue>
              </Progress>
              <p className="text-caption text-muted-foreground tabular-nums">
                {t(($) => $.workspace.limits.usage_breakdown, {
                  used: numberFormatter.format(quotaUsage.used),
                  reserved: numberFormatter.format(quotaUsage.reserved),
                })}
              </p>
              {quotaResetAt ? (
                <p className="text-caption text-muted-foreground tabular-nums">
                  {t(($) => $.workspace.limits.resets_at, {
                    date: quotaResetAt,
                  })}
                </p>
              ) : null}
            </div>
          ) : (
            <div className="flex flex-col gap-2 sm:items-end">
              {entitlements.limits.automationRuns.mode === "limited" &&
              entitlements.limits.automationRuns.limit !== null ? (
                <span className="tabular-nums">
                  {t(($) => $.workspace.limits.per_month, {
                    count: entitlements.limits.automationRuns.limit,
                  })}
                </span>
              ) : null}
              <span className="text-caption text-muted-foreground">
                {t(($) => $.workspace.limits.usage_unavailable)}
              </span>
              <Button
                type="fill"
                size="small"
                icon={RefreshCw}
                aria-label={t(($) => $.workspace.actions.retry_automations)}
                loading={quotaUsageQuery.isFetching}
                disabled={quotaUsageQuery.isFetching}
                onClick={() => void quotaUsageQuery.refetch()}
              >
                {t(($) => $.workspace.actions.retry)}
              </Button>
            </div>
          )}
        </SettingsFormRow>
      </SettingsGroup>

      {hasSeatCapacity || summaryUnavailable ? (
        <SettingsGroup
          title={t(($) => $.workspace.seats.title)}
          description={t(($) => $.workspace.seats.description)}
        >
          {summaryUnavailable ? (
            <Alert
              className="mb-3"
              type="secondary"
              icon={AlertCircle}
              title={t(($) => $.workspace.seats.summary_unavailable_title)}
              description={t(
                ($) => $.workspace.seats.summary_unavailable_description,
              )}
              action={
                <Button
                  type="fill"
                  size="small"
                  icon={RefreshCw}
                  loading={summaryQuery.isFetching}
                  disabled={summaryQuery.isFetching}
                  onClick={() => void summaryQuery.refetch()}
                >
                  {t(($) => $.workspace.actions.retry)}
                </Button>
              }
            />
          ) : null}
          {seatPurchaseMessage ? (
            <Alert
              className="mb-3"
              type="success"
              icon={CheckCircle2}
              title={t(($) => $.workspace.seats.updated)}
              description={seatPurchaseMessage}
            />
          ) : null}
          {membersExceedPurchasedSeats ? (
            <Alert
              className="mb-3"
              type="secondary"
              icon={AlertCircle}
              title={t(($) => $.workspace.seats.members_over_capacity_title)}
              description={t(
                ($) => $.workspace.seats.members_over_capacity_description,
                {
                  actual: actualSeats,
                  purchased: billedSeats,
                },
              )}
              action={
                canAddSeats ? (
                  <Button
                    type="fill"
                    size="small"
                    icon={Plus}
                    disabled={isMutating}
                    onClick={() => handleSeatPurchaseOpenChange(true)}
                  >
                    {t(($) => $.workspace.actions.add_seats)}
                  </Button>
                ) : null
              }
            />
          ) : null}
          {activeSeatPurchase ? (
            <Alert
              className="mb-3"
              type={seatPurchasePollingTimedOut ? "warning" : "info"}
              icon={seatPurchasePollingTimedOut ? AlertCircle : Loader2}
              classNames={{
                icon: seatPurchasePollingTimedOut
                  ? undefined
                  : "animate-spin motion-reduce:animate-none",
              }}
              title={
                seatPurchasePollingTimedOut
                  ? t(($) => $.workspace.seat_purchase.delayed_title)
                  : t(($) => $.workspace.seat_purchase.pending_title)
              }
              description={
                seatPurchasePollingTimedOut
                  ? activeSeatPurchaseExpiry
                    ? t(
                        ($) =>
                          $.workspace.seat_purchase
                            .delayed_description_with_expiry,
                        { date: activeSeatPurchaseExpiry },
                      )
                    : t(($) => $.workspace.seat_purchase.delayed_description)
                  : t(($) => $.workspace.seat_purchase.pending_description, {
                      count: activeSeatPurchase.targetSeats,
                    })
              }
            />
          ) : null}
          <SettingsFormRow
            label={t(($) => $.workspace.seats.human_members)}
            description={t(($) => $.workspace.seats.human_members_description)}
          >
            <span className="tabular-nums">
              {t(($) => $.workspace.seats.seat_count, {
                count: usedSeats,
              })}
            </span>
          </SettingsFormRow>
          <SettingsFormRow
            divider
            label={t(($) => $.workspace.seats.billed)}
            description={t(($) => $.workspace.seats.billed_description)}
          >
            {summaryQuery.isPending ? (
              <Skeleton
                className="h-5 w-20 motion-reduce:animate-none"
                aria-label={t(($) => $.workspace.seats.summary_loading)}
              />
            ) : summaryUnavailable ? (
              <span className="text-muted-foreground">
                {t(($) => $.workspace.seats.unavailable)}
              </span>
            ) : billedSeats === null ? (
              <span className="text-muted-foreground">
                {t(($) => $.workspace.seats.not_subscribed)}
              </span>
            ) : (
              <div className="flex flex-col gap-2 sm:items-end">
                <span className="tabular-nums">
                  {t(($) => $.workspace.seats.seat_count, {
                    count: billedSeats,
                  })}
                </span>
                {canAddSeats ? (
                  <Button
                    className="w-full sm:w-auto"
                    size="large"
                    type="primary"
                    icon={Plus}
                    disabled={isMutating}
                    onClick={() => handleSeatPurchaseOpenChange(true)}
                  >
                    {t(($) => $.workspace.actions.add_seats)}
                  </Button>
                ) : null}
              </div>
            )}
          </SettingsFormRow>
          <SettingsFormRow
            divider
            label={t(($) => $.workspace.seats.pending_invitations)}
            description={t(
              ($) => $.workspace.seats.pending_invitations_description,
            )}
          >
            {summaryQuery.isPending ? (
              <Skeleton
                className="h-5 w-20 motion-reduce:animate-none"
                aria-label={t(($) => $.workspace.seats.summary_loading)}
              />
            ) : summaryUnavailable ? (
              <span className="text-muted-foreground">
                {t(($) => $.workspace.seats.unavailable)}
              </span>
            ) : (
              <span className="tabular-nums">
                {t(($) => $.workspace.seats.seat_count, {
                  count: reservedSeats,
                })}
              </span>
            )}
          </SettingsFormRow>
          <SettingsFormRow
            divider
            label={t(($) => $.workspace.seats.available)}
            description={t(($) => $.workspace.seats.available_description)}
          >
            {summaryQuery.isPending ? (
              <Skeleton
                className="h-5 w-20 motion-reduce:animate-none"
                aria-label={t(($) => $.workspace.seats.summary_loading)}
              />
            ) : summaryUnavailable || availableSeats === null ? (
              <span className="text-muted-foreground">
                {t(($) => $.workspace.seats.unavailable)}
              </span>
            ) : (
              <span className="tabular-nums">
                {t(($) => $.workspace.seats.seat_count, {
                  count: availableSeats,
                })}
              </span>
            )}
          </SettingsFormRow>
          <SettingsFormRow
            divider
            label={t(($) => $.workspace.seats.pending)}
            description={t(($) => $.workspace.seats.pending_description)}
          >
            {summaryQuery.isPending ? (
              <Skeleton
                className="h-5 w-28 motion-reduce:animate-none"
                aria-label={t(($) => $.workspace.seats.summary_loading)}
              />
            ) : summaryUnavailable ? (
              <span className="text-muted-foreground">
                {t(($) => $.workspace.seats.unavailable)}
              </span>
            ) : pendingSeatQuantity === null ? (
              <span className="text-muted-foreground">
                {t(($) => $.workspace.seats.none_pending)}
              </span>
            ) : summaryPeriodEnd ? (
              <span className="tabular-nums">
                {t(($) => $.workspace.seats.pending_with_date, {
                  count: pendingSeatQuantity,
                  date: summaryPeriodEnd,
                })}
              </span>
            ) : (
              <span className="tabular-nums">
                {t(($) => $.workspace.seats.seat_count, {
                  count: pendingSeatQuantity,
                })}
              </span>
            )}
          </SettingsFormRow>
        </SettingsGroup>
      ) : null}

      <Modal
        open={seatPurchaseOpen}
        title={t(($) => $.workspace.seat_purchase.title)}
        width={512}
        onCancel={() => handleSeatPurchaseOpenChange(false)}
        footer={
          <div className="flex items-center justify-end gap-2">
            <Button
              type="fill"
              disabled={purchaseSeatsMutation.isPending}
              onClick={() => handleSeatPurchaseOpenChange(false)}
            >
              {t(($) => $.workspace.actions.cancel)}
            </Button>
            <Button
              type="primary"
              icon={Plus}
              loading={purchaseSeatsMutation.isPending}
              disabled={
                !seatPreview ||
                formattedSeatProration === null ||
                formattedNextSeatInvoice === null ||
                previewSeatPurchaseMutation.isPending ||
                seatPreviewRefreshing ||
                purchaseSeatsMutation.isPending
              }
              onClick={() => void handleSeatPurchase()}
            >
              {seatPreview
                ? t(($) => $.workspace.seat_purchase.confirm, {
                    count: seatPreview.additionalSeats,
                  })
                : t(($) => $.workspace.actions.add_seats)}
            </Button>
          </div>
        }
      >
        <div className="flex flex-col gap-5">
          <p className="text-body text-muted-foreground">
            {t(($) => $.workspace.seat_purchase.description)}
          </p>
          <div className="space-y-4">
            <div className="space-y-2">
              <label
                className="text-body font-medium"
                htmlFor="workspace-additional-seats"
              >
                {t(($) => $.workspace.seat_purchase.additional_label)}
              </label>
              <Input
                id="workspace-additional-seats"
                type="number"
                inputMode="numeric"
                min={1}
                step={1}
                value={additionalSeatsInput}
                disabled={purchaseSeatsMutation.isPending}
                onChange={(event) => {
                  seatPreviewCapacityRetryRef.current = null;
                  setSeatPreviewRefreshing(false);
                  setAdditionalSeatsInput(event.currentTarget.value);
                  setSeatPurchaseError(null);
                }}
              />
              <p className="text-caption text-muted-foreground">
                {t(($) => $.workspace.seat_purchase.additional_hint)}
              </p>
            </div>
            {previewSeatPurchaseMutation.isPending || seatPreviewRefreshing ? (
              <div
                className="flex items-center gap-2 text-body text-muted-foreground"
                role="status"
              >
                <Loader2 className="animate-spin motion-reduce:animate-none" />
                {t(($) => $.workspace.seat_purchase.preview_loading)}
              </div>
            ) : null}
            {seatPurchaseError ? (
              <Alert
                type="error"
                icon={AlertCircle}
                title={t(($) => $.workspace.seat_purchase.error_title)}
                description={seatPurchaseError}
              />
            ) : null}
            {seatPreview &&
            formattedSeatProration !== null &&
            formattedNextSeatInvoice !== null ? (
              <div className="space-y-3 rounded-lg border border-surface-border p-4">
                <div className="flex items-center justify-between gap-4 text-body">
                  <span className="text-muted-foreground">
                    {t(($) => $.workspace.seat_purchase.seats_after)}
                  </span>
                  <span className="font-medium tabular-nums">
                    {t(($) => $.workspace.seats.seat_count, {
                      count: seatPreview.resultingSeats,
                    })}
                  </span>
                </div>
                <div className="flex items-center justify-between gap-4 text-body">
                  <span className="text-muted-foreground">
                    {t(($) => $.workspace.seat_purchase.charge_today)}
                  </span>
                  <span className="font-medium tabular-nums">
                    {formattedSeatProration}
                  </span>
                </div>
                <div className="flex items-center justify-between gap-4 text-body">
                  <span className="text-muted-foreground">
                    {t(($) => $.workspace.seat_purchase.next_invoice)}
                  </span>
                  <span className="font-medium tabular-nums">
                    {formattedNextSeatInvoice}
                  </span>
                </div>
                <p className="text-caption leading-5 text-muted-foreground">
                  {t(($) => $.workspace.seat_purchase.tax_notice)}
                </p>
              </div>
            ) : null}
          </div>
        </div>
      </Modal>

      <Modal
        open={checkoutConfirmOpen}
        title={t(($) => $.workspace.confirm.title)}
        width={512}
        closable={false}
        maskClosable={false}
        onCancel={() => handleCheckoutConfirmOpenChange(false)}
        footer={
          <div className="flex items-center justify-end gap-2">
            <Button
              type="fill"
              disabled={checkoutMutation.isPending}
              onClick={() => handleCheckoutConfirmOpenChange(false)}
            >
              {t(($) => $.workspace.actions.cancel)}
            </Button>
            <Button
              type="primary"
              icon={ExternalLink}
              loading={checkoutMutation.isPending}
              disabled={checkoutMutation.isPending}
              onClick={() => void handleCheckout()}
            >
              {t(($) => $.workspace.actions.continue_to_stripe)}
            </Button>
          </div>
        }
      >
        <p className="text-body text-muted-foreground">
          {t(($) => $.workspace.confirm.description, {
            interval:
              interval === "month"
                ? t(($) => $.workspace.upgrade.monthly)
                : t(($) => $.workspace.upgrade.yearly),
            count: actualSeats,
          })}
        </p>
      </Modal>
    </div>
  );
}
