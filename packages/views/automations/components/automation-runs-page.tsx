"use client";

import { useCallback, useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { workspaceAutomationRunsOptions } from "@patchbay/core/automations/queries";
import { useWorkspaceId } from "@patchbay/core/hooks";
import { useWorkspacePaths } from "@patchbay/core/paths";
import { Button } from "@patchbay/ui/components/ui/button";
import { Skeleton } from "@patchbay/ui/components/ui/skeleton";
import { Tabs, TabsList, TabsTrigger } from "@patchbay/ui/components/ui/tabs";
import { AppLink } from "../../navigation";
import { useT } from "../../i18n";
import { RunHistoryList } from "./automation-detail-page";

export function AutomationRunsPage() {
  const { t } = useT("automations");
  const wsId = useWorkspaceId();
  const paths = useWorkspacePaths();
  const [scope, setScope] = useState<"mine" | "team">("team");
  const [offset, setOffset] = useState(0);
  const [filters, setFilters] = useState({
    search: "",
    statuses: [] as string[],
  });
  const onFiltersChange = useCallback((search: string, statuses: string[]) => {
    setFilters({ search, statuses });
    setOffset(0);
  }, []);
  const { data, isPending, isFetching, error, refetch } = useQuery(
    workspaceAutomationRunsOptions(wsId, { scope, offset, ...filters }),
  );
  const automationMap = useMemo(
    () =>
      Object.fromEntries(
        (data?.runs ?? []).map((run) => [
          run.automation_id,
          { title: run.automation_title, executor_id: run.executor_id },
        ]),
      ),
    [data?.runs],
  );
  const cards = [
    "successful_24h",
    "failed_24h",
    "successful_7d",
    "failed_7d",
  ] as const;
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex h-11 shrink-0 items-center gap-2 border-b px-5 text-body">
        <AppLink
          href={paths.automations()}
          className="text-muted-foreground hover:text-foreground"
        >
          {t(($) => $.page.title)}
        </AppLink>
        <span className="text-muted-foreground">/</span>
        <span>{t(($) => $.overview.runs)}</span>
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto">
        <div className="mx-auto w-full max-w-5xl space-y-6 px-5 py-10 sm:px-8">
          <h1 className="text-2xl font-semibold">
            {t(($) => $.overview.runs)}
          </h1>

          {error ? (
            <div
              role="alert"
              className="flex items-center justify-between gap-3 rounded-lg border p-4 text-destructive"
            >
              <span>
                {error instanceof Error ? error.message : String(error)}
              </span>
              <Button
                variant="outline"
                size="sm"
                onClick={() => void refetch()}
              >
                {t(($) => $.page.retry)}
              </Button>
            </div>
          ) : null}
          <div aria-busy={isFetching}>
            <RunHistoryList
              runs={data?.runs ?? []}
              loading={isPending}
              agentId=""
              triggers={[]}
              automations={automationMap}
              onFiltersChange={onFiltersChange}
              beforeTable={
                <div className="grid grid-cols-2 gap-3 sm:grid-cols-4">
                  {cards.map((key) => (
                    <div
                      key={key}
                      className="rounded-xl border bg-background px-4 py-4"
                    >
                      <p className="text-caption text-muted-foreground">
                        {t(($) => $.overview[key])}
                      </p>
                      {isPending ? (
                        <Skeleton className="mt-3 h-7 w-12" />
                      ) : (
                        <p className="mt-3 text-title font-medium tabular-nums">
                          {error ? "—" : (data?.summary[key] ?? 0)}
                        </p>
                      )}
                    </div>
                  ))}
                </div>
              }
              toolbarStart={
                <Tabs
                  value={scope}
                  onValueChange={(value) => {
                    setScope(value as "mine" | "team");
                    setOffset(0);
                  }}
                >
                  <TabsList>
                    <TabsTrigger value="mine">
                      {t(($) => $.overview.mine)}
                    </TabsTrigger>
                    <TabsTrigger value="team">
                      {t(($) => $.overview.team)}
                    </TabsTrigger>
                  </TabsList>
                </Tabs>
              }
            />
          </div>
          {(data?.summary.total ?? 0) > 25 || offset > 0 ? (
            <div className="flex items-center justify-end gap-3 text-caption">
              <Button
                variant="ghost"
                size="sm"
                disabled={offset === 0 || isFetching}
                onClick={() => setOffset(Math.max(0, offset - 25))}
              >
                {t(($) => $.overview.previous)}
              </Button>
              <span>
                {Math.floor(offset / 25) + 1} /{" "}
                {Math.max(1, Math.ceil((data?.summary.total ?? 0) / 25))}
              </span>
              <Button
                variant="ghost"
                size="sm"
                disabled={
                  isFetching || offset + 25 >= (data?.summary.total ?? 0)
                }
                onClick={() => setOffset(offset + 25)}
              >
                {t(($) => $.overview.next)}
              </Button>
            </div>
          ) : null}
        </div>
      </div>
    </div>
  );
}
