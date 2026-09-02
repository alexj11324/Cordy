"use client";

import { useMemo, useState } from "react";
import { useInfiniteQuery } from "@tanstack/react-query";
import { Plus } from "lucide-react";
import { toast } from "sonner";
import {
  issueWorkProductsInfiniteOptions,
  useCreateWorkProductRelation,
  useDetachWorkProductRelation,
  workProductListInfiniteOptions,
} from "@patchbay/core/work-products";
import { useWorkspaceId } from "@patchbay/core/hooks";
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
import { WorkProductRow } from "./work-product-row";
import { useT } from "../i18n";

/**
 * The issue's delivery list — the only one. Pull requests used to have a
 * section of their own above this, reading a different endpoint backed by
 * different tables; a PR could appear in one and not the other and nothing in
 * the UI explained why. Both now come from `/work-products`, so a product's
 * presence here is the same fact the server's close gate reads.
 */
export function WorkProductRelationsSection({ issueId }: { issueId: string }) {
  const wsId = useWorkspaceId();
  const { t } = useT("work-products");
  const [dialogOpen, setDialogOpen] = useState(false);
  const [selectedProductId, setSelectedProductId] = useState("");
  const [closeIntent, setCloseIntent] = useState(false);
  const productsQuery = useInfiniteQuery(issueWorkProductsInfiniteOptions(wsId, issueId));
  const availableProductsQuery = useInfiniteQuery(
    workProductListInfiniteOptions(wsId, undefined, dialogOpen),
  );
  const createRelation = useCreateWorkProductRelation();
  const detachRelation = useDetachWorkProductRelation();
  const products = useMemo(
    () => productsQuery.data?.pages.flatMap((page) => page.work_products) ?? [],
    [productsQuery.data],
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

  function detach(relationId: string) {
    if (detachRelation.isPending) return;
    detachRelation.mutate(
      { issueId, relationId },
      {
        onSuccess: () => toast.success(t(($) => $.relations.detach_success)),
        onError: () => toast.error(t(($) => $.relations.detach_error)),
      },
    );
  }

  return (
    <section className="space-y-2" aria-labelledby={`work-product-relations-${issueId}`}>
      <div className="flex items-center justify-between gap-2">
        <h3 id={`work-product-relations-${issueId}`} className="text-caption font-medium">
          {t(($) => $.relations.title)}
          {products.length > 0 ? (
            <span className="ml-1 text-muted-foreground">· {products.length}</span>
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

      {productsQuery.isPending ? (
        <p className="px-2 text-caption text-muted-foreground">{t(($) => $.relations.loading)}</p>
      ) : products.length === 0 ? (
        <p className="px-2 text-caption text-muted-foreground">{t(($) => $.relations.empty)}</p>
      ) : (
        <div className="space-y-1">
          {products.map((product) => (
            <WorkProductRow
              key={product.relation.id || product.id}
              product={product}
              onDetach={detach}
              detachPending={detachRelation.isPending}
            />
          ))}
        </div>
      )}

      {productsQuery.hasNextPage ? (
        <Button
          size="sm"
          variant="ghost"
          disabled={productsQuery.isFetchingNextPage}
          onClick={() => void productsQuery.fetchNextPage()}
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
