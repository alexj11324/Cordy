"use client";

import { useRouter } from "next/navigation";
import { useEffect } from "react";

/**
 * Legacy OAuth callback route.
 * With Clerk, OAuth is handled internally. This page simply redirects
 * to the home page (Clerk will have already established the session).
 */
export default function AuthCallbackPage() {
  const router = useRouter();

  useEffect(() => {
    router.replace("/");
  }, [router]);

  return null;
}
