// @vitest-environment node

import { describe, expect, it, vi } from "vitest";
import {
  createAuthHandoffDelivery,
  type AuthHandoffPayload,
} from "./auth-handoff";

const payload: AuthHandoffPayload = {
  code: `pbd_${"a".repeat(43)}`,
  state: "b".repeat(43),
};

async function flushDelivery(): Promise<void> {
  await Promise.resolve();
  await Promise.resolve();
}

describe("native auth handoff delivery", () => {
  it("retains an unacknowledged handoff for an explicit retry", async () => {
    const callback = vi
      .fn<(value: AuthHandoffPayload) => Promise<boolean>>()
      .mockResolvedValueOnce(false)
      .mockResolvedValueOnce(true);
    const delivery = createAuthHandoffDelivery(callback);

    delivery.enqueue(payload);
    await flushDelivery();
    expect(callback).toHaveBeenCalledTimes(1);

    delivery.retry();
    await flushDelivery();
    expect(callback).toHaveBeenCalledTimes(2);
    expect(callback).toHaveBeenNthCalledWith(2, payload);

    delivery.retry();
    await flushDelivery();
    expect(callback).toHaveBeenCalledTimes(2);
  });

  it("acknowledges only successful delivery", async () => {
    const callback = vi
      .fn<(value: AuthHandoffPayload) => Promise<boolean>>()
      .mockResolvedValueOnce(false)
      .mockResolvedValueOnce(true);
    const acknowledge = vi.fn();
    const delivery = createAuthHandoffDelivery(callback, acknowledge);

    delivery.enqueue(payload);
    await flushDelivery();
    expect(acknowledge).not.toHaveBeenCalled();

    delivery.retry();
    await flushDelivery();
    expect(acknowledge).toHaveBeenCalledOnce();
    expect(acknowledge).toHaveBeenCalledWith(payload);
  });

  it("delivers queued handoffs in order after each acknowledgement", async () => {
    const secondPayload = { ...payload, state: "c".repeat(43) };
    const delivered: AuthHandoffPayload[] = [];
    const callback = vi.fn(async (value: AuthHandoffPayload) => {
      delivered.push(value);
      return true;
    });
    const delivery = createAuthHandoffDelivery(callback);

    delivery.enqueue(payload);
    delivery.enqueue(secondPayload);
    await flushDelivery();

    expect(delivered).toEqual([payload, secondPayload]);
  });

  it("does not deliver after disposal", async () => {
    const callback = vi.fn(async () => true);
    const delivery = createAuthHandoffDelivery(callback);

    delivery.enqueue(payload);
    delivery.dispose();
    await flushDelivery();

    expect(callback).not.toHaveBeenCalled();
  });
});
