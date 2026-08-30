"use client";

import { useRef, type CSSProperties, type ReactNode } from "react";
import { ArrowLeft } from "lucide-react";
import { cn } from "@patchbay/ui/lib/utils";
import { useScrollFade } from "@patchbay/ui/hooks/use-scroll-fade";
import { Button } from "@patchbay/ui/components/ui/button";
import { Card, CardContent } from "@patchbay/ui/components/ui/card";
import { DragStrip } from "@patchbay/views/platform";
import type { OnboardingStep } from "@patchbay/core/onboarding";
import { StepProgressBar, StepSidebar } from "./step-sidebar";

/**
 * Geometry for the onboarding steps, taken from the ReUI onboarding-3 block so
 * the two panes read as one design.
 *
 * The old model had three competing measures — a 920px frame, a 620px column
 * and an in-frame cap — which is how the platform fork ended up ~150px off the
 * left edge of every other step. There is now one measure. A step that needs
 * to break out of it says so locally rather than picking a different global.
 */
export const STEP_COLUMN =
  "mx-auto flex min-h-full w-full max-w-[28rem] flex-col";
export const STEP_GUTTER = "px-6 pb-8 pt-14 sm:px-10 md:pt-8 lg:px-14 lg:py-10";

/**
 * Title + supporting line for a step.
 *
 * Replaces a serif display headline over a grey uppercase eyebrow. The block
 * leads with a plain semibold sans title and one muted line under it, and the
 * eyebrow has no equivalent there — with the rail naming the step, a third
 * label for the same screen was redundant.
 *
 * `aria-live` because the panes persist across steps: a screen reader user
 * moving between steps gets no navigation event, so the heading has to
 * announce itself.
 */
export function StepHeading({
  title,
  description,
}: {
  title: ReactNode;
  description?: ReactNode;
}) {
  return (
    <div className="flex flex-col gap-1.5" aria-live="polite">
      <h1 className="text-balance text-title-lg font-semibold text-foreground">
        {title}
      </h1>
      {description ? (
        <p className="text-pretty text-body text-muted-foreground">
          {description}
        </p>
      ) : null}
    </div>
  );
}

/**
 * The step's actions, pinned to the bottom of the column.
 *
 * Full-width stacked buttons rather than a right-aligned inline row: the
 * column is 28rem, so an inline bar leaves the primary action floating in the
 * middle of the screen instead of landing where the eye finishes the form.
 * `mt-auto` against the column's `min-h-full` is what pins it.
 */
export function StepFooter({
  children,
  hint,
}: {
  children: ReactNode;
  /** Optional status line above the buttons (validation, progress). */
  hint?: ReactNode;
}) {
  return (
    <div className="mt-auto flex flex-col gap-2 pb-2 pt-10">
      {hint ? (
        <p aria-live="polite" className="text-caption text-muted-foreground">
          {hint}
        </p>
      ) : null}
      {children}
    </div>
  );
}

/**
 * The frame every onboarding step renders into: progress rail on the left,
 * the step's own content scrolling on the right.
 *
 * It owns the window, not just a header strip. That is what lets the four
 * steps stop repeating an identical wrapper / DragStrip / header / `<main>`
 * preamble, including the scroll-fade wiring that was duplicated verbatim in
 * all four and is now set up once.
 *
 * The column is `min-h-full` inside a scrolling pane rather than centred by
 * the pane: `items-center` on a scroll container clips the top of anything
 * taller than the viewport, and these steps do overflow on short windows.
 */
export function StepShell({
  currentStep,
  onBack,
  backLabel,
  backDisabled,
  onStepChange,
  chromeFooter,
  singlePane = false,
  children,
}: {
  currentStep: OnboardingStep;
  onBack?: () => void;
  backLabel?: string;
  /** Workspace step disables Back while its create request is in flight. */
  backDisabled?: boolean;
  /** Return to an already-completed step from the rail. */
  onStepChange?: (step: OnboardingStep) => void;
  /** Injected by the flow — the Log out escape hatch. Rendered in whichever
   *  chrome is visible: the rail at `md` and up, the compact bar below it. */
  chromeFooter?: ReactNode;
  /** Web-only presentation: replace the desktop progress rail with one
   *  centred shadcn card. Desktop keeps the persistent rail by default. */
  singlePane?: boolean;
  children: ReactNode;
}) {
  const mainRef = useRef<HTMLElement>(null);
  const fadeStyle = useScrollFade(mainRef);

  if (singlePane) {
    return (
      <div className="animate-onboarding-enter flex h-full min-h-0 flex-col bg-muted/20">
        <main
          ref={mainRef}
          style={fadeStyle}
          className="min-h-0 flex-1 overflow-y-auto px-4 py-5 sm:px-6 sm:py-8 lg:py-10"
        >
          <Card className="mx-auto min-h-full w-full max-w-2xl gap-0 py-0 shadow-sm">
            <CardContent className="flex min-h-full flex-1 flex-col px-6 py-6 sm:px-10 sm:py-8 lg:px-12 lg:py-10">
              {onBack || chromeFooter ? (
                <div className="mb-6 flex min-h-8 items-center justify-between gap-4">
                  {onBack ? (
                    <Button
                      type="button"
                      variant="ghost"
                      size="icon"
                      onClick={onBack}
                      disabled={backDisabled}
                      aria-label={backLabel}
                      className="-ml-2"
                      style={{ WebkitAppRegion: "no-drag" } as CSSProperties}
                    >
                      <ArrowLeft aria-hidden="true" />
                    </Button>
                  ) : (
                    <span />
                  )}
                  {chromeFooter ? (
                    <div className="shrink-0">{chromeFooter}</div>
                  ) : null}
                </div>
              ) : null}
              {children}
            </CardContent>
          </Card>
        </main>
      </div>
    );
  }

  return (
    <div className="animate-onboarding-enter flex h-full min-h-0 flex-col bg-background">
      {/* Compact desktop windows do not render the rail, so they keep a
          native-only top-edge drag target and place their controls below it.
          At md+ the rail owns the titlebar surface instead. */}
      <div className="md:hidden">
        <DragStrip />
      </div>

      <div className="flex min-h-0 flex-1">
        <StepSidebar
          currentStep={currentStep}
          onBack={onBack}
          backDisabled={backDisabled}
          onStepChange={onStepChange}
          footer={chromeFooter}
        />

        <main
          ref={mainRef}
          style={fadeStyle}
          className={cn("min-h-0 min-w-0 flex-1 overflow-y-auto", STEP_GUTTER)}
        >
          <div className={STEP_COLUMN}>
            <StepProgressBar
              currentStep={currentStep}
              onBack={onBack}
              backDisabled={backDisabled}
              footer={chromeFooter}
            />
            {children}
          </div>
        </main>
      </div>
    </div>
  );
}
