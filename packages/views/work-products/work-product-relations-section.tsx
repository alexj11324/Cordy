"use client";

import { useMemo, useState } from "react";
import { useInfiniteQuery, useQueries } from "@tanstack/react-query";
import { ChevronRight, FileText, Plus } from "lucide-react";
import { toast } from "sonner";
import {
  useCreateWorkProductRelation,
  workProductDetailOptions,
  workProductListInfiniteOptions,
  workProductRelationsInfiniteOptions,
} from "@patchbay/core/work-products";
import { useWorkspacePaths } from "@patchbay/core/paths";
import { useWorkspaceId } from "@patchbay/core/hooks";
import type { WorkProduct } from "@patchbay/core/types";
import { Badge } from "@patchbay/ui/components/ui/badge";
import { Button } from "@patchbay/ui/components/ui/button";
import { Checkbox } from "@patchbay/ui/components/ui/checkbox";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@patchbay/ui/components/ui/dialog";
import { Label } from "@patchbay/ui/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@patchbay/ui/components/ui/select";
import { AppLink } from "../navigation";
import { useT } from "../i18n";

export function WorkProductRelationsSection({ issueId }: { issueId: string }) {
  const wsId = useWorkspaceId();
  const paths = useWorkspacePaths();
  const { t } = useT("work-products");
  const [dialogOpen, setDialogOpen] = useState(false);
  const [selectedProductId, setSelectedProductId] = useState("");
  const [closeIntent, setCloseIntent] = useState(false);
  const relationsQuery = useInfiniteQuery(
    workProductRelationsInfiniteOptions(wsId, issueId),
  );
  const relatedIds = useMemo(
    () => {
      const seen = new Set<string>();
      return (relationsQuery.data?.pages.flatMap((page) =>
        page.relations.map((relation) => relation.work_product_id),
      ) ?? []).filter((productId) => {
        if (!productId || seen.has(productId)) return false;
        seen.add(productId);
        return true;
      });
    },
    [relationsQuery.data],
  );
  const detailQueries = useQueries({
    queries: relatedIds.map((productId) => workProductDetailOptions(wsId, productId)),
  });
  const productsById = useMemo(() => {
    const result = new Map<string, WorkProduct>();
    for (const query of detailQueries) {
      if (query.data?.id) result.set(query.data.id, query.data);
    }
    return result;
  }, [detailQueries]);
  const availableProductsQuery = useInfiniteQuery(
    workProductListInfiniteOptions(wsId, undefined, dialogOpen),
  );
  const createRelation = useCreateWorkProductRelation();
  const relations = useMemo(
    () => relationsQuery.data?.pages.flatMap((page) => page.relations) ?? [],
    [relationsQuery.data],
  );
  const availableProducts = useMemo(() => {
    const seen = new Set<string>();
    return (availableProductsQuery.data?.pages.flatMap((page) => page.products) ?? []).filter(
      (product) => {
        if (!product.id || seen.has(product.id)) return false;
        seen.add(product.id);
        return true;
      },
    );
  }, [availableProductsQuery.data]);
  const selectItems = availableProducts.map((product) => ({
    value: product.id,
    label: product.external_identity || product.id,
  }));

  function closeDialog() {
    setDialogOpen(false);
    setSelectedProductId("");
    setCloseIntent(false);
  }

  function submit() {
    if (!selectedProductId || createRelation.isPending) return;
    createRelation.mutate(
      {
        issueId,
        work_product_id: selectedProductId,
        close_intent: closeIntent,
      },
      {
        onSuccess: () => {
          closeDialog();
          toast.success(t(($) => $.relations.success));
        },
        onError: () => toast.error(t(($) => $.relations.error)),
      },
    );
  }

  return (
    <section className="space-y-2" aria-labelledby={`work-product-relations-${issueId}`}>
      <div className="flex items-center justify-between gap-2">
        <h3 id={`work-product-relations-${issueId}`} className="text-caption font-medium">
          {t(($) => $.relations.title)}
          {relations.length > 0 ? (
            <span className="ml-1 text-muted-foreground">· {relations.length}</span>
          ) : null}
        </h3>
        <Button
          size="icon-sm"
          variant="ghost"
          aria-label={t(($) => $.relations.attach)}
          title={t(($) => $.relations.attach)}
          onClick={() => setDialogOpen(true)}
        >
          <Plus aria-hidden="true" />
        </Button>
      </div>

      {relationsQuery.isPending ? (
        <p className="px-2 text-caption text-muted-foreground">{t(($) => $.relations.loading)}</p>
      ) : relations.length === 0 ? (
        <p className="px-2 text-caption text-muted-foreground">{t(($) => $.relations.empty)}</p>
      ) : (
        <div className="space-y-1">
          {relations.map((relation) => {
            const product = productsById.get(relation.work_product_id);
            return (
              <div key={relation.id} className="group flex min-w-0 items-center gap-1 rounded-md px-2 py-1.5 hover:bg-accent/50">
                <FileText aria-hidden="true" className="size-3.5 shrink-0 text-muted-foreground" />
                <AppLink
                  href={paths.workProductDetail(relation.work_product_id)}
                  className="min-w-0 flex-1 truncate text-caption hover:text-foreground"
                >
                  {product?.external_identity || relation.work_product_id}
                </AppLink>
                {relation.close_intent ? (
                  <Badge variant="secondary">{t(($) => $.relations.close_intent_short)}</Badge>
                ) : null}
                <ChevronRight aria-hidden="true" className="size-3 shrink-0 text-muted-foreground" />
              </div>
            );
          })}
        </div>
      )}

      {relationsQuery.hasNextPage ? (
        <Button
          size="sm"
          variant="ghost"
          disabled={relationsQuery.isFetchingNextPage}
          onClick={() => void relationsQuery.fetchNextPage()}
        >
          {t(($) => $.page.load_more)}
        </Button>
      ) : null}

      <Dialog
        open={dialogOpen}
        onOpenChange={(open) => {
          if (open) setDialogOpen(true);
          else closeDialog();
        }}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{t(($) => $.relations.dialog_title)}</DialogTitle>
            <DialogDescription>{t(($) => $.relations.select_label)}</DialogDescription>
          </DialogHeader>
          <div className="space-y-4">
            {availableProductsQuery.isPending ? (
              <p className="text-caption text-muted-foreground">{t(($) => $.relations.loading)}</p>
            ) : availableProducts.length === 0 ? (
              <p className="text-caption text-muted-foreground">{t(($) => $.relations.no_products)}</p>
            ) : (
              <div className="space-y-1.5">
                <Label htmlFor={`work-product-select-${issueId}`}>
                  {t(($) => $.relations.select_label)}
                </Label>
                <Select
                  items={selectItems}
                  value={selectedProductId || null}
                  onValueChange={(value) => setSelectedProductId(value ?? "")}
                >
                  <SelectTrigger id={`work-product-select-${issueId}`} className="w-full">
                    <SelectValue placeholder={t(($) => $.relations.select_placeholder)} />
                  </SelectTrigger>
                  <SelectContent>
                    {availableProducts.map((product) => (
                      <SelectItem key={product.id} value={product.id}>
                        {product.external_identity || product.id}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>
            )}
            {availableProductsQuery.hasNextPage ? (
              <Button
                size="sm"
                variant="ghost"
                disabled={availableProductsQuery.isFetchingNextPage}
                onClick={() => void availableProductsQuery.fetchNextPage()}
              >
                {t(($) => $.page.load_more)}
              </Button>
            ) : null}
            <label className="flex items-start gap-2 text-caption text-muted-foreground">
              <Checkbox checked={closeIntent} onCheckedChange={(value) => setCloseIntent(value === true)} />
              <span>{t(($) => $.relations.close_intent)}</span>
            </label>
          </div>
          <DialogFooter>
            <Button variant="outline" onClick={closeDialog}>
              {t(($) => $.relations.cancel)}
            </Button>
            <Button disabled={!selectedProductId || createRelation.isPending} onClick={submit}>
              {t(($) => $.relations.link)}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </section>
  );
}
