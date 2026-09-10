"use client";

import { Suspense } from "react";
import { DashboardLayout } from "@orvilo/views/layout";
import { OrviloIcon } from "@orvilo/ui/components/common/orvilo-icon";
import { SearchCommand, SearchTrigger } from "@orvilo/views/search";
import { WebNotificationBridge } from "@/components/web-notification-bridge";
import { WorkspaceDocumentTitle } from "@/platform/workspace-document-title";

export default function Layout({ children }: { children: React.ReactNode }) {
  return (
    <>
      {/* useSearchParams requires a Suspense boundary in the app router. Sits
          outside DashboardLayout so the tab is named while the guard is still
          resolving the workspace. */}
      <Suspense fallback={null}>
        <WorkspaceDocumentTitle />
      </Suspense>
      <DashboardLayout
        loadingIndicator={<OrviloIcon className="size-6" />}
        searchSlot={<SearchTrigger />}
        extra={
          <>
            <SearchCommand />
            <WebNotificationBridge />
          </>
        }
      >
        {children}
      </DashboardLayout>
    </>
  );
}
