"use client";

import { ErrorBoundary } from "@patchbay/ui/components/common/error-boundary";
import { TaskGraphPage } from "@patchbay/views/task-graph";

export default function Page() {
  return (
    <ErrorBoundary>
      <TaskGraphPage />
    </ErrorBoundary>
  );
}
