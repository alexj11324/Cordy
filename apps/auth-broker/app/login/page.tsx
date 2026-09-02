"use client";
import { Suspense, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useAuth } from "@clerk/nextjs";
import { useSearchParams } from "next/navigation";
import { AuthShell } from "@/components/auth-shell";
import { completeDesktopGoogleAttempt } from "@/lib/broker-client";
import { buildDesktopCallbackUrl, readDesktopHandoffBinding } from "@/lib/desktop-handoff";
import { useAuthMessages } from "@/lib/auth-messages";
export default function Page() { return <Suspense><Content /></Suspense>; }
function Content() { const params = useSearchParams(); const binding = useMemo(() => readDesktopHandoffBinding(params), [params]); const { isLoaded, isSignedIn, getToken } = useAuth(); const messages = useAuthMessages(); const attempted = useRef(false); const [error, setError] = useState(false);
  const complete = useCallback(async () => { if (!binding) { setError(true); return; } try { const token = await getToken(); if (!token) throw new Error(); const result = await completeDesktopGoogleAttempt(token, { state: binding.state, code_challenge: binding.codeChallenge }); window.location.assign(buildDesktopCallbackUrl(result.code, binding.state, result.callbackProtocol)); } catch { setError(true); } }, [binding, getToken]);
  useEffect(() => { if (!binding) { setError(true); return; } if (!isLoaded || attempted.current) return; attempted.current = true; if (!isSignedIn) { window.location.replace(`/oauth/google?${binding.query}`); return; } void complete(); }, [binding, complete, isLoaded, isSignedIn]);
  return <AuthShell><p role={error ? "alert" : "status"}>{error ? messages.desktopFailed : messages.opening}</p>{binding && error && <button onClick={() => void complete()}>{messages.open}</button>}</AuthShell>; }
