"use client";

import { use } from "react";
import { IssueDetailRoute } from "@orvilo/views/issues/components";
import { ErrorBoundary } from "@orvilo/ui/components/common/error-boundary";

export default function IssueDetailPage({
  params,
}: {
  params: Promise<{ id: string }>;
}) {
  const { id } = use(params);
  return (
    <ErrorBoundary resetKeys={[id]}>
      <IssueDetailRoute routeId={id} />
    </ErrorBoundary>
  );
}
