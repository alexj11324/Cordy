"use client";

import { GitBranch, Hash, Server } from "lucide-react";
import type { ReactNode } from "react";
import { Badge } from "@patchbay/ui/components/ui/badge";
import { Card, CardContent } from "@patchbay/ui/components/ui/card";
import { AppLink } from "../navigation";
import { useLocale, useT } from "../i18n";
import { useWorkspacePaths } from "@patchbay/core/paths";
import type { ExecutionProvenance } from "@patchbay/core/types";

function formatDate(value: string | null, locale: string): string {
  if (!value) return "—";
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? value : date.toLocaleString(locale);
}

function valueOrUnknown(value: string | null, unknownLabel: string): string {
  return value?.trim() || unknownLabel;
}

export function ProvenanceCard({ provenance }: { provenance: ExecutionProvenance }) {
  const { t } = useT("work-products");
  const locale = useLocale();
  const paths = useWorkspacePaths();
  const status = valueOrUnknown(provenance.discovery_status, t(($) => $.provenance.unknown));
  const productId = provenance.discovery_work_product_id;

  return (
    <Card size="sm" data-testid="provenance-card">
      <CardContent className="space-y-3">
        <div className="flex min-w-0 items-start justify-between gap-3">
          <div className="flex min-w-0 items-center gap-2">
            <Server aria-hidden="true" className="size-4 shrink-0 text-muted-foreground" />
            <span className="truncate font-mono text-caption text-muted-foreground">
              {provenance.repo_identity || t(($) => $.provenance.unknown)}
            </span>
          </div>
          <Badge variant="outline">{status}</Badge>
        </div>

        <dl className="grid min-w-0 gap-x-4 gap-y-2 text-caption sm:grid-cols-2">
          <ProvenanceField
            icon={<GitBranch aria-hidden="true" className="size-3.5" />}
            label={t(($) => $.provenance.branch)}
            value={valueOrUnknown(provenance.head_branch, t(($) => $.provenance.unknown))}
          />
          <ProvenanceField
            icon={<Hash aria-hidden="true" className="size-3.5" />}
            label={t(($) => $.provenance.sha)}
            value={valueOrUnknown(provenance.head_sha, t(($) => $.provenance.unknown))}
            mono
          />
          <ProvenanceField
            label={t(($) => $.provenance.workspace)}
            value={valueOrUnknown(provenance.execution_workspace, t(($) => $.provenance.unknown))}
            mono
          />
          <ProvenanceField
            label={t(($) => $.provenance.matches)}
            value={String(provenance.discovery_match_count)}
          />
          <ProvenanceField
            label={t(($) => $.provenance.reason)}
            value={valueOrUnknown(provenance.discovery_reason, t(($) => $.provenance.unknown))}
          />
          <ProvenanceField
            label={t(($) => $.detail.updated)}
            value={formatDate(provenance.updated_at, locale)}
          />
        </dl>

        {productId ? (
          <AppLink
            href={paths.workProductDetail(productId)}
            className="inline-flex max-w-full items-center text-caption text-muted-foreground underline decoration-muted-foreground/40 underline-offset-4 hover:text-foreground"
          >
            {t(($) => $.detail.title)}
          </AppLink>
        ) : null}
      </CardContent>
    </Card>
  );
}

function ProvenanceField({
  icon,
  label,
  value,
  mono = false,
}: {
  icon?: ReactNode;
  label: string;
  value: string;
  mono?: boolean;
}) {
  return (
    <div className="min-w-0">
      <dt className="flex items-center gap-1 text-muted-foreground">
        {icon}
        {label}
      </dt>
      <dd className={`mt-0.5 break-words text-foreground ${mono ? "font-mono" : ""}`}>
        {value}
      </dd>
    </div>
  );
}
