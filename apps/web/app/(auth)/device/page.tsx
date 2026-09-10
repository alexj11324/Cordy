"use client";

import { useEffect } from "react";
import { useAuthStore } from "@orvilo/core/auth";
import { DeviceAuthorization } from "@orvilo/views/auth";
import { AuthShell } from "@/components/auth-shell";
import { useWebRouter } from "@/platform/client-navigation";

export default function DeviceAuthorizationPage() {
  const status = useAuthStore((state) => state.status);
  const router = useWebRouter();

  useEffect(() => {
    if (status === "unauthenticated") {
      router.replace("/login?redirect_url=%2Fdevice");
    }
  }, [router, status]);

  return (
    <AuthShell>
      {status === "authenticated" ? (
        <DeviceAuthorization />
      ) : (
        <span role="status">…</span>
      )}
    </AuthShell>
  );
}
