"use client";

import { TaskGraphPage } from "@orvilo/views/task-graph";
import { ErrorBoundary } from "@orvilo/ui/components/common/error-boundary";

export default function Page() {
  return (
    <ErrorBoundary>
      <TaskGraphPage />
    </ErrorBoundary>
  );
}
