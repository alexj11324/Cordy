import { useQueryClient } from "@tanstack/react-query";
import { dependencyGraphKeys } from "@/data/queries/dependency-graphs";
import { useWSSubscriptions } from "@/lib/use-ws-subscriptions";

export function useDependencyGraphsRealtime() {
  const queryClient = useQueryClient();

  useWSSubscriptions(
    (ws, wsId) => {
      const invalidate = () => {
        void queryClient.invalidateQueries({
          queryKey: dependencyGraphKeys.all(wsId),
        });
      };

      return [ws.on("dependency_graph:updated", invalidate), ws.onReconnect(invalidate)];
    },
    [queryClient],
  );
}
