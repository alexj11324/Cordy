// @vitest-environment node

import { describe, expect, it } from "vitest";
import type {
  AutomationQuotaUsage,
  WorkspaceSubscriptionEntitlements,
  WorkspaceSubscriptionSummary,
} from "@patchbay/core/types";
import {
  hasActiveWorkspaceSeatCapacity,
  resolveAutomationUsage,
} from "./billing-state";

const freeEntitlements: WorkspaceSubscriptionEntitlements = {
  workspaceId: "workspace-1",
  plan: "free",
  status: "inactive",
  seats: 3,
  limits: {
    issueCount: { mode: "limited", limit: 17 },
    automationRuns: { mode: "limited", limit: 7 },
  },
  currentPeriodEnd: null,
  snapshotExpiresAt: null,
  version: 1,
};

const quotaUsage: AutomationQuotaUsage = {
  action: "enforce",
  used: 3,
  reserved: 2,
  total: 5,
  limit: 7,
  reached: false,
  period_start: "2030-01-01T00:00:00Z",
  period_end: "2030-02-01T00:00:00Z",
  reset_at: "2030-02-01T00:00:00Z",
  blocked_counts: {},
};

describe("resolveAutomationUsage", () => {
  it("counts reserved runs toward progress and the reached decision", () => {
    expect(
      resolveAutomationUsage(freeEntitlements, quotaUsage, false),
    ).toEqual({
      kind: "metered",
      used: 3,
      reserved: 2,
      total: 5,
      limit: 7,
      progress: 500 / 7,
      reached: false,
      resetAt: "2030-02-01T00:00:00Z",
    });

    expect(
      resolveAutomationUsage(
        freeEntitlements,
        { ...quotaUsage, used: 5, total: 7, reached: true },
        false,
      ),
    ).toMatchObject({ total: 7, reached: true, progress: 100 });
  });

  it("renders the server's explicit unlimited mode", () => {
    expect(
      resolveAutomationUsage(
        {
          ...freeEntitlements,
          limits: {
            ...freeEntitlements.limits,
            automationRuns: { mode: "unlimited", limit: null },
          },
        },
        undefined,
        true,
      ),
    ).toEqual({ kind: "unlimited" });
  });

  it("does not turn missing or disabled limited usage into zero or unlimited", () => {
    expect(
      resolveAutomationUsage(freeEntitlements, undefined, true),
    ).toEqual({ kind: "unavailable" });
    expect(
      resolveAutomationUsage(
        freeEntitlements,
        {
          ...quotaUsage,
          action: "off",
          used: null,
          reserved: null,
          total: null,
          limit: null,
          reached: null,
          reset_at: null,
        },
        false,
      ),
    ).toEqual({ kind: "unavailable" });
    expect(
      resolveAutomationUsage(
        freeEntitlements,
        {
          ...quotaUsage,
          action: "observe",
          total: quotaUsage.limit,
          reached: null,
        },
        false,
      ),
    ).toEqual({ kind: "unavailable" });
  });

  it("uses the explicit limited mode even when plan and status say Pro", () => {
    expect(
      resolveAutomationUsage(
        {
          ...freeEntitlements,
          plan: "pro",
          status: "active",
        },
        quotaUsage,
        false,
      ),
    ).toMatchObject({ kind: "metered", limit: 7 });
  });
});

describe("billing subscription state", () => {
  it("keeps billing history separate from current seat capacity", () => {
    const canceledSummary = {
      entitlement: freeEntitlements,
      billingInterval: null,
      humanMembers: 3,
      seatCapacity: null,
      cancelAtPeriodEnd: false,
      graceUntil: null,
      hasStripeCustomer: true,
      availableActions: {
        checkout: true,
        portal: true,
        purchaseSeats: false,
      },
    } satisfies WorkspaceSubscriptionSummary;

    expect(hasActiveWorkspaceSeatCapacity(canceledSummary)).toBe(false);

    const activeSummary = {
      ...canceledSummary,
      seatCapacity: {
        purchased: 5,
        used: 3,
        reserved: 1,
        available: 1,
        overcommitted: false,
        version: 2,
        pendingQuantity: null,
        activePurchase: null,
      },
    } satisfies WorkspaceSubscriptionSummary;
    expect(hasActiveWorkspaceSeatCapacity(activeSummary)).toBe(true);
  });
});
