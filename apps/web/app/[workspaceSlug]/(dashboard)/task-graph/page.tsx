"use client";

import { TaskGraphPage } from "@patchbay/views/task-graph";
import { ErrorBoundary } from "@patchbay/ui/components/common/error-boundary";

export default function Page() {
  return (
    <ErrorBoundary>
      <TaskGraphPage />
    </ErrorBoundary>
  );
}
