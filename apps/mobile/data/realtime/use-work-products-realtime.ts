/**
 * Work Product realtime is mounted for the whole workspace session because
 * the catalog is a global mobile surface. Provider webhooks and explicit
 * attach/detach operations all publish pull_request:updated; invalidating the
 * workspace product list makes the next foreground read converge without
 * adding per-record subscriptions to screens that do not render relations.
 */
import { useQueryClient } from "@tanstack/react-query";
import { workProductKeys } from "@/data/queries/work-products";
import { useWSSubscriptions } from "@/lib/use-ws-subscriptions";

export function useWorkProductsRealtime() {
  const qc = useQueryClient();

  useWSSubscriptions(
    (ws, wsId) => {
      const invalidate = () =>
        qc.invalidateQueries({ queryKey: workProductKeys.all(wsId) });
      return [ws.on("pull_request:updated", invalidate), ws.onReconnect(invalidate)];
    },
    [qc],
  );
}
