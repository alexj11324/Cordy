"use client";
import { Suspense, useEffect, useMemo, useRef, useState } from "react";
import { useClerk, useSignIn } from "@clerk/nextjs";
import { useSearchParams } from "next/navigation";
import { AuthShell } from "@/components/auth-shell";
import { registerDesktopGoogleAttempt } from "@/lib/broker-client";
import { readDesktopHandoffBinding } from "@/lib/desktop-handoff";
import { hasClerkOAuthReturn, readGoogleSso, startGoogleOAuth } from "@/lib/google-oauth";
import { useAuthMessages } from "@/lib/auth-messages";
export default function Page() { return <Suspense><Content /></Suspense>; }
function Content() { const params = useSearchParams(); const binding = useMemo(() => readDesktopHandoffBinding(params), [params]); const clerk = useClerk(); const { signIn } = useSignIn(); const messages = useAuthMessages(); const started = useRef(false); const [registered, setRegistered] = useState(false); const [error, setError] = useState(false);
  useEffect(() => { if (started.current || error) return; if (!binding) { setError(true); return; } if (!clerk.loaded) return; if (hasClerkOAuthReturn(params, window.location.hash)) { started.current = true; window.location.replace(`/oauth/google/callback${window.location.search}${window.location.hash}`); return; } if (!registered) { started.current = true; void registerDesktopGoogleAttempt({ state: binding.state, code_challenge: binding.codeChallenge }).then(() => { started.current = false; setRegistered(true); }).catch(() => setError(true)); return; } if (clerk.session) { started.current = true; void clerk.signOut({ sessionId: clerk.session.id, redirectUrl: `/oauth/google?${binding.query}` }).catch(() => setError(true)); return; } if (!readGoogleSso(signIn)) return; started.current = true; void startGoogleOAuth(signIn, window.location.origin, binding.query).then(({ error: failure }) => { if (failure) setError(true); }).catch(() => setError(true)); }, [binding, clerk, error, params, registered, signIn]);
  return <AuthShell><p role={error ? "alert" : "status"}>{error ? messages.startFailed : messages.starting}</p>{error && <button onClick={() => window.location.reload()}>{messages.retry}</button>}</AuthShell>; }
