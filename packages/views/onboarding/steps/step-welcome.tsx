"use client";

import { useState } from "react";
import { ArrowRight, Download, Loader2 } from "lucide-react";
import { Button, buttonVariants } from "@patchbay/ui/components/ui/button";
import { PatchbayIcon } from "@patchbay/ui/components/common/patchbay-icon";
import { DragStrip } from "@patchbay/views/platform";
import { useT } from "../../i18n";

/**
 * Step 0 — the one-shot product intro shown on every onboarding
 * entry (which-step-are-you-on is not persisted). Returning users
 * who are already onboarded never reach this screen; they're gated
 * out earlier by `!hasOnboarded`.
 *
 * `onSkip`, when provided, renders a secondary ghost CTA that marks
 * onboarding complete server-side and sends the user straight to
 * their existing workspace. OnboardingFlow only passes it when the
 * user has ≥ 1 workspace — without that, skipping lands in limbo.
 *
 * `isWeb` flips two things when true: the subheading acknowledges
 * that web users have an extra device step, and a "Download Desktop"
 * secondary CTA surfaces before the user has invested in workspace
 * setup. Desktop bundles a daemon, so the same prompt would be noise
 * there.
 */
export function StepWelcome({
  onNext,
  onSkip,
  isWeb = false,
}: {
  onNext: () => void | Promise<void>;
  onSkip?: () => void | Promise<void>;
  isWeb?: boolean;
}) {
  const { t } = useT("onboarding");
  // Tracks which button is mid-flight so we can show a per-button
  // spinner and disable both while one is in progress.
  const [pending, setPending] = useState<"next" | "skip" | null>(null);

  const handleNext = async () => {
    if (pending) return;
    setPending("next");
    try {
      await onNext();
    } finally {
      setPending(null);
    }
  };

  const handleSkip = async () => {
    if (pending || !onSkip) return;
    setPending("skip");
    try {
      await onSkip();
    } finally {
      setPending(null);
    }
  };

  return (
    <div className="dark animate-onboarding-enter flex h-full min-h-[640px] flex-col bg-black font-[system-ui,-apple-system,BlinkMacSystemFont,'Segoe_UI',sans-serif]">
      <DragStrip />
      <div className="flex flex-1 flex-col justify-center px-6 pb-12 sm:px-10 md:px-20">
        <div className="mx-auto flex w-full max-w-[540px] flex-col items-center gap-8 text-center">
          <div className="flex items-center justify-center gap-3">
            <PatchbayIcon className="size-8 text-foreground" noSpin />
            <span className="text-display font-medium tracking-tight sm:text-4xl">
              {t(($) => $.welcome.wordmark)}
            </span>
          </div>

          <h1 className="text-balance text-5xl font-medium leading-[1.04] tracking-tight sm:text-6xl">
            {t(($) => $.welcome.headline)}
          </h1>

          <div className="flex flex-col gap-4">
            <p className="text-title leading-relaxed text-foreground">
              {t(($) => $.welcome.lede)}
            </p>
            <p className="text-body leading-relaxed text-muted-foreground">
              {isWeb
                ? t(($) => $.welcome.lede_web)
                : t(($) => $.welcome.lede_desktop)}
            </p>
          </div>

          <div className="flex flex-wrap items-center justify-center gap-3">
            {isWeb ? (
              <>
                {/* `<a>` rather than `<Button onClick={window.open}>`
                    so middle-click / cmd-click / "Copy link" all
                    behave and screen readers announce it as a link
                    (it navigates; `Continue on web` is the button
                    that mutates flow state). New tab preserves this
                    onboarding tab in case the desktop install
                    stalls and the user falls back here. */}
                <a
                  href="/download"
                  target="_blank"
                  rel="noopener noreferrer"
                  className={buttonVariants({ size: "lg" })}
                >
                  <Download className="h-4 w-4" />
                  {t(($) => $.welcome.download_desktop)}
                </a>
                <Button
                  size="lg"
                  variant="outline"
                  onClick={handleNext}
                  disabled={pending !== null}
                >
                  {pending === "next" && (
                    <Loader2 className="h-4 w-4 animate-spin" />
                  )}
                  {t(($) => $.welcome.continue_on_web)}
                  <ArrowRight className="h-4 w-4" />
                </Button>
              </>
            ) : (
              <Button
                size="lg"
                onClick={handleNext}
                disabled={pending !== null}
              >
                {pending === "next" && (
                  <Loader2 className="h-4 w-4 animate-spin" />
                )}
                {t(($) => $.welcome.start_exploring)}
                <ArrowRight className="h-4 w-4" />
              </Button>
            )}
            {onSkip && (
              <Button
                size="lg"
                variant="ghost"
                onClick={handleSkip}
                disabled={pending !== null}
              >
                {pending === "skip" && (
                  <Loader2 className="h-4 w-4 animate-spin" />
                )}
                {t(($) => $.welcome.skip_existing)}
              </Button>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
