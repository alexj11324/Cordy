export type AuthHandoffPayload = {
  code: string;
  state: string;
};

type AuthHandoffCallback = (
  payload: AuthHandoffPayload,
) => boolean | Promise<boolean>;

/**
 * Keeps a deep-link payload until the renderer acknowledges it. The native
 * one-time code is retained across transient API failures and retried when
 * the browser reports connectivity again; the verifier remains renderer-only.
 */
export function createAuthHandoffDelivery(
  callback: AuthHandoffCallback,
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
      if (acknowledged && pending[0] === payload) pending.shift();
    } catch {
      // Keep the payload for an explicit retry after a transient failure.
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
