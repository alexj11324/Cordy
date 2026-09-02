"use client";

import { useMemo } from "react";
import { useInfiniteQuery, useQuery } from "@tanstack/react-query";
import { ArrowLeft, ExternalLink, FileText } from "lucide-react";
import {
  workProductDetailOptions,
  workProductProvenanceInfiniteOptions,
} from "@patchbay/core/work-products";
import { useWorkspacePaths } from "@patchbay/core/paths";
import { useWorkspaceId } from "@patchbay/core/hooks";
import { Button } from "@patchbay/ui/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@patchbay/ui/components/ui/card";
import { Skeleton } from "@patchbay/ui/components/ui/skeleton";
import { CollectionPageHeader, CollectionPageState } from "../layout/collection-page";
import { AppLink } from "../navigation";
import { useLocale, useT } from "../i18n";
import { ProvenanceCard } from "./provenance-card";

export function WorkProductDetailPage({ id }: { id: string }) {
  const wsId = useWorkspaceId();
  const paths = useWorkspacePaths();
  const locale = useLocale();
  const { t } = useT("work-products");
  const productQuery = useQuery(workProductDetailOptions(wsId, id));
  const provenanceQuery = useInfiniteQuery(workProductProvenanceInfiniteOptions(wsId));
  const product = productQuery.data;
  const provenance = useMemo(
    () =>
      (provenanceQuery.data?.pages.flatMap((page) => page.provenance) ?? []).filter(
        (item) => item.discovery_work_product_id === id,
      ),
    [id, provenanceQuery.data],
  );

  if (productQuery.isPending) {
    return <DetailSkeleton />;
  }

  if (productQuery.isError || !product?.id) {
    return (
      <CollectionPageState
        icon={FileText}
        title={t(($) => $.detail.not_found)}
        tone="destructive"
        role="alert"
        actions={
          <Button size="sm" variant="outline" render={<AppLink href={paths.workProducts()} />}>
            <ArrowLeft aria-hidden="true" className="size-3.5" />
            {t(($) => $.detail.back)}
          </Button>
        }
      />
    );
  }

  return (
    <div className="relative flex min-h-0 flex-1 flex-col">
      <CollectionPageHeader
        icon={FileText}
        title={product.external_identity || t(($) => $.detail.title)}
        actions={
          <Button
            size="sm"
            variant="ghost"
            render={<AppLink href={paths.workProducts()} />}
          >
            <ArrowLeft aria-hidden="true" className="size-3.5" />
            <span className="hidden sm:inline">{t(($) => $.detail.back)}</span>
          </Button>
        }
      />

      <div className="min-h-0 flex-1 overflow-auto px-4 pb-8 sm:px-6">
        <div className="mx-auto max-w-4xl space-y-6 py-4">
          <Card>
            <CardHeader>
              <CardTitle>{t(($) => $.detail.title)}</CardTitle>
            </CardHeader>
            <CardContent>
              <dl className="grid gap-x-6 gap-y-4 text-caption sm:grid-cols-2">
                <DetailField label={t(($) => $.detail.external_identity)} value={product.external_identity} mono />
                <DetailField label={t(($) => $.detail.provider)} value={product.provider} />
                <DetailField label={t(($) => $.detail.kind)} value={product.kind} />
                <DetailField label={t(($) => $.detail.created)} value={formatDate(product.created_at, locale)} />
                <DetailField label={t(($) => $.detail.updated)} value={formatDate(product.updated_at, locale)} />
              </dl>
              {product.external_url ? (
                <a
                  href={product.external_url}
                  target="_blank"
                  rel="noopener noreferrer"
                  className="mt-5 inline-flex items-center gap-1.5 text-caption text-muted-foreground underline decoration-muted-foreground/40 underline-offset-4 hover:text-foreground"
                >
                  <ExternalLink aria-hidden="true" className="size-3.5" />
                  {t(($) => $.detail.open_external)}
                </a>
              ) : null}
            </CardContent>
          </Card>

          <section className="space-y-3" aria-labelledby="work-product-detail-provenance-heading">
            <h2 id="work-product-detail-provenance-heading" className="text-body font-medium">
              {t(($) => $.detail.provenance)}
            </h2>
            {provenanceQuery.isPending ? (
              <Skeleton className="h-28 w-full" />
            ) : provenance.length === 0 ? (
              <p className="text-caption text-muted-foreground">
                {t(($) => $.detail.provenance_empty)}
              </p>
            ) : (
              provenance.map((item) => (
                <ProvenanceCard key={`${item.task_id}:${item.repo_identity}:${item.execution_workspace}`} provenance={item} />
              ))
            )}
          </section>
        </div>
      </div>
    </div>
  );
}

function DetailField({ label, value, mono = false }: { label: string; value: string; mono?: boolean }) {
  return (
    <div className="min-w-0">
      <dt className="text-muted-foreground">{label}</dt>
      <dd className={`mt-0.5 break-words text-foreground ${mono ? "font-mono" : ""}`}>
        {value || "—"}
      </dd>
    </div>
  );
}

function formatDate(value: string, locale: string): string {
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? value || "—" : date.toLocaleString(locale);
}

function DetailSkeleton() {
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <CollectionPageHeader icon={FileText} title={<Skeleton className="h-4 w-32" />} />
      <div className="mx-auto w-full max-w-4xl space-y-4 px-4 py-6 sm:px-6">
        <Skeleton className="h-40 w-full" />
        <Skeleton className="h-28 w-full" />
      </div>
    </div>
  );
}
