export type AuthHandoffPayload = {
  code: string;
  state: string;
};

type AuthHandoffCallback = (
  payload: AuthHandoffPayload,
) => boolean | Promise<boolean>;
type AuthHandoffAcknowledger = (payload: AuthHandoffPayload) => void;

/**
 * Keeps native handoffs until the renderer acknowledges redemption. A failed
 * redemption is retried by the preload's existing browser-online recovery
 * signal; the one-time code itself remains protected by the Rust verifier.
 */
export function createAuthHandoffDelivery(
  callback: AuthHandoffCallback,
  acknowledge?: AuthHandoffAcknowledger,
): {
  enqueue: (payload: AuthHandoffPayload) => void;
  retry: () => void;
  dispose: () => void;
} {
  let pending: AuthHandoffPayload[] = [];
  let deliveryInFlight = false;
  let disposed = false;

  const deliver = async (): Promise<void> => {
    if (disposed || deliveryInFlight) return;
    const payload = pending[0];
    if (!payload) return;

    deliveryInFlight = true;
    let acknowledged = false;
    try {
      acknowledged = await callback(payload);
      if (acknowledged) {
        if (pending[0] === payload) pending.shift();
        acknowledge?.(payload);
      }
    } catch {
      // Keep the payload for the next explicit retry signal.
    } finally {
      deliveryInFlight = false;
      if (acknowledged && pending.length > 0) void deliver();
    }
  };

  return {
    enqueue: (payload) => {
      pending.push(payload);
      queueMicrotask(() => void deliver());
    },
    retry: () => void deliver(),
    dispose: () => {
      disposed = true;
      pending = [];
    },
  };
}
