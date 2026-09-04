"use client";

import { Suspense, useEffect, useMemo, useRef, useState } from "react";
import { useClerk, useSignIn, useSignUp } from "@clerk/nextjs";
import { useRouter, useSearchParams } from "next/navigation";
import { AuthShell } from "@/components/auth-shell";
import { readDesktopHandoffBinding } from "@/lib/desktop-handoff";
import { consumeGoogleOAuthNonce, googleOAuthAttemptIsReady } from "@/lib/google-oauth";
import { useAuthMessages } from "@/lib/auth-messages";
import { resolveStandaloneReturnUrl } from "@/lib/redirect";

export default function Page() { return <Suspense><Content /></Suspense>; }
function Content() {
  const params = useSearchParams();
  const binding = useMemo(() => readDesktopHandoffBinding(params), [params]);
  const desktopRequest = params.get("platform") === "desktop";
  const returnUrl = useMemo(() => {
    if (binding) return `/login?${binding.query}`;
    return resolveStandaloneReturnUrl(
      params.get("return_url") ?? params.get("redirect_url"),
    );
  }, [binding, params]);
  const clerk = useClerk(); const { signIn } = useSignIn(); const { signUp } = useSignUp(); const router = useRouter(); const messages = useAuthMessages(); const attempted = useRef(false); const [error, setError] = useState(false);
  useEffect(() => {
    if (desktopRequest && !binding) { setError(true); return; }
    if (!clerk.loaded || attempted.current || !signIn || !signUp) return;
    const destination = returnUrl; const fail = () => setError(true);
    const navigate = (url: string) => /^https?:\/\//.test(url) ? window.location.assign(url) : router.replace(url);
    type Options = NonNullable<Parameters<typeof signIn.finalize>[0]>;
    const onNavigate: NonNullable<Options["navigate"]> = async ({ session, decorateUrl }) => { if (session?.currentTask) return fail(); navigate(decorateUrl(destination)); };
    const run = async () => {
      if (!await consumeGoogleOAuthNonce(signIn, params.get("rotating_token_nonce"))) return;
      if (!googleOAuthAttemptIsReady(signIn, signUp)) return;
      attempted.current = true;
      if (signIn.status === "complete") { const result = await signIn.finalize({ navigate: onNavigate }); if (result.error) fail(); return; }
      if (signIn.isTransferable) { const transfer = await signUp.create({ transfer: true }); if (transfer.error || (signUp.status as string) !== "complete") return fail(); const result = await signUp.finalize({ navigate: onNavigate }); if (result.error) fail(); return; }
      if (signUp.isTransferable) { const transfer = await signIn.create({ transfer: true }); if (transfer.error || (signIn.status as string) !== "complete") return fail(); const result = await signIn.finalize({ navigate: onNavigate }); if (result.error) fail(); return; }
      const session = signIn.existingSession?.sessionId ?? signUp.existingSession?.sessionId;
      if (!session) return fail();
      await clerk.setActive({ session, navigate: async ({ session: active, decorateUrl }) => { if (active?.currentTask) return fail(); navigate(decorateUrl(destination)); } });
    };
    void run().catch(fail);
  }, [binding, clerk, desktopRequest, params, returnUrl, router, signIn, signUp]);
  return <AuthShell><p role={error ? "alert" : "status"}>{error ? messages.completeFailed : messages.completing}</p><div id="clerk-captcha" /></AuthShell>;
}
