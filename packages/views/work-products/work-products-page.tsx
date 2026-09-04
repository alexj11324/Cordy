"use client";

import { useMemo } from "react";
import { useInfiniteQuery } from "@tanstack/react-query";
import { ChevronRight, FileText } from "lucide-react";
import {
  workProductListInfiniteOptions,
  workProductProvenanceInfiniteOptions,
} from "@patchbay/core/work-products";
import { useWorkspacePaths } from "@patchbay/core/paths";
import { useWorkspaceId } from "@patchbay/core/hooks";
import type { WorkProduct } from "@patchbay/core/types";
import { Button } from "@patchbay/ui/components/ui/button";
import { Card, CardContent } from "@patchbay/ui/components/ui/card";
import { Skeleton } from "@patchbay/ui/components/ui/skeleton";
import {
  CollectionPageHeader,
  CollectionPageState,
} from "../layout/collection-page";
import { AppLink } from "../navigation";
import { useLocale, useT } from "../i18n";
import { ProvenanceCard } from "./provenance-card";

export function WorkProductsPage() {
  const wsId = useWorkspaceId();
  const paths = useWorkspacePaths();
  const { t } = useT("work-products");
  const locale = useLocale();
  const productsQuery = useInfiniteQuery(workProductListInfiniteOptions(wsId));
  const provenanceQuery = useInfiniteQuery(workProductProvenanceInfiniteOptions(wsId));

  const products = useMemo(() => {
    const seen = new Set<string>();
    return (productsQuery.data?.pages.flatMap((page) => page.products) ?? []).filter((product) => {
      if (!product.id || seen.has(product.id)) return false;
      seen.add(product.id);
      return true;
    });
  }, [productsQuery.data]);
  const provenance = useMemo(
    () => provenanceQuery.data?.pages.flatMap((page) => page.provenance) ?? [],
    [provenanceQuery.data],
  );

  return (
    <div className="relative flex min-h-0 flex-1 flex-col">
      <CollectionPageHeader
        icon={FileText}
        title={t(($) => $.page.title)}
        count={products.length}
        description={t(($) => $.page.description)}
      />

      <div className="min-h-0 flex-1 overflow-auto px-4 pb-8 sm:px-6">
        {productsQuery.isPending ? (
          <ProductListSkeleton />
        ) : productsQuery.isError ? (
          <CollectionPageState
            icon={FileText}
            title={t(($) => $.page.error)}
            tone="destructive"
            role="alert"
            actions={
              <Button size="sm" variant="outline" onClick={() => void productsQuery.refetch()}>
                {t(($) => $.page.retry)}
              </Button>
            }
          />
        ) : products.length === 0 ? (
          <CollectionPageState icon={FileText} title={t(($) => $.page.empty)} />
        ) : (
          <div className="mx-auto max-w-4xl space-y-8 py-4">
            <div className="space-y-2" data-testid="work-product-list">
              {products.map((product) => (
                <WorkProductRow
                  key={product.id}
                  product={product}
                  href={paths.workProductDetail(product.id)}
                  locale={locale}
                />
              ))}
            </div>

            {productsQuery.hasNextPage ? (
              <div className="flex justify-center">
                <Button
                  size="sm"
                  variant="outline"
                  disabled={productsQuery.isFetchingNextPage}
                  onClick={() => void productsQuery.fetchNextPage()}
                >
                  {t(($) => $.page.load_more)}
                </Button>
              </div>
            ) : null}

            <section className="space-y-3" aria-labelledby="work-product-provenance-heading">
              <h2
                id="work-product-provenance-heading"
                className="text-body font-medium"
              >
                {t(($) => $.page.provenance_title)}
              </h2>
              {provenanceQuery.isPending ? (
                <Skeleton className="h-28 w-full" />
              ) : provenance.length === 0 ? (
                <p className="text-caption text-muted-foreground">
                  {t(($) => $.page.provenance_empty)}
                </p>
              ) : (
                <div className="space-y-2">
                  {provenance.map((item) => (
                    <ProvenanceCard key={`${item.task_id}:${item.repo_identity}:${item.execution_workspace}`} provenance={item} />
                  ))}
                </div>
              )}
            </section>
          </div>
        )}
      </div>
    </div>
  );
}

function WorkProductRow({
  product,
  href,
  locale,
}: {
  product: WorkProduct;
  href: string;
  locale: string;
}) {
  const { t } = useT("work-products");
  return (
    <Card size="sm" className="transition-colors hover:bg-surface-hover">
      <CardContent className="p-0">
        <AppLink
          href={href}
          className="flex min-w-0 items-center gap-3 px-3 py-3 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
        >
          <FileText aria-hidden="true" className="size-4 shrink-0 text-muted-foreground" />
          <div className="min-w-0 flex-1">
            <div className="truncate font-medium text-body">
              {product.external_identity || t(($) => $.provenance.unknown)}
            </div>
            <div className="mt-0.5 flex flex-wrap gap-x-2 gap-y-0.5 text-caption text-muted-foreground">
              <span>{product.provider || t(($) => $.provenance.unknown)}</span>
              <span aria-hidden="true">·</span>
              <span>{product.kind || t(($) => $.provenance.unknown)}</span>
              <span aria-hidden="true">·</span>
              <time dateTime={product.updated_at}>
                {formatDate(product.updated_at, locale)}
              </time>
            </div>
          </div>
          <ChevronRight aria-hidden="true" className="size-4 shrink-0 text-muted-foreground" />
        </AppLink>
      </CardContent>
    </Card>
  );
}

function formatDate(value: string, locale: string): string {
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? value || "—" : date.toLocaleDateString(locale);
}

function ProductListSkeleton() {
  return (
    <div className="mx-auto max-w-4xl space-y-2 py-4" aria-hidden="true">
      {Array.from({ length: 4 }, (_, index) => (
        <Card size="sm" key={index}>
          <CardContent className="flex items-center gap-3">
            <Skeleton className="size-4 rounded" />
            <div className="flex-1 space-y-2">
              <Skeleton className="h-4 w-1/2" />
              <Skeleton className="h-3 w-1/3" />
            </div>
          </CardContent>
        </Card>
      ))}
    </div>
  );
}
