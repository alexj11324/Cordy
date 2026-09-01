"use client";

import { Suspense } from "react";
import { DashboardLayout } from "@patchbay/views/layout";
import { PatchbayIcon } from "@patchbay/ui/components/common/patchbay-icon";
import { SearchCommand, SearchTrigger } from "@patchbay/views/search";
import { FloatingChat } from "@patchbay/views/chat";
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
        loadingIndicator={<PatchbayIcon className="size-6" />}
        searchSlot={<SearchTrigger />}
        extra={
          <>
            <SearchCommand />
            <WebNotificationBridge />
            <FloatingChat />
          </>
        }
      >
        {children}
      </DashboardLayout>
    </>
  );
}
