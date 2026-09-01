"use client";

import {
  Item,
  ItemContent,
  ItemDescription,
  ItemMedia,
  ItemTitle,
} from "@patchbay/ui/components/ui/item";
import { useT } from "../../i18n";

/** Mirrors `patrickAgentAvatarURL` in server/internal/handler/patrick_agent.go.
 *  Placeholder until Patrick has real artwork — these two must move together or
 *  onboarding shows one face and the created agent another. */
export const PATRICK_PLACEHOLDER_EMOJI = "🦄";
import { StepHeading } from "./step-shell";

/**
 * The subject of step 3, stated as an object rather than a footnote.
 *
 * The step used to be titled after its dependency — "Pick an agent runtime" —
 * with Patrick named only in the grey lede. That put the thing being created in
 * the small print and the technical prerequisite in the headline, so a member
 * reached a button saying "Start with Patrick" without ever being told who that
 * is. Headline plus a card with a mark on it is what makes the introduction
 * survive not reading carefully.
 *
 * Patrick does not exist yet at this point in the flow — she is created when the
 * member commits — so this cannot render her stored avatar. It reuses the mark
 * the Runtimes page already uses for "Start with Patrick" instead, which keeps
 * the two entry points recognisably the same thing.
 */
export function PatrickIntro() {
  const { t } = useT("onboarding");
  return (
    <div className="flex flex-col gap-5">
      <StepHeading title={t(($) => $.patrick_intro.headline)} />
      <Item variant="outline">
        <ItemMedia>
          <span
            role="img"
            aria-label={t(($) => $.patrick_intro.name)}
            className="flex size-9 shrink-0 select-none items-center justify-center rounded-full bg-muted text-title leading-none"
          >
            {PATRICK_PLACEHOLDER_EMOJI}
          </span>
        </ItemMedia>
        <ItemContent>
          <ItemTitle>
            {t(($) => $.patrick_intro.name)}
            <span className="text-muted-foreground">
              {" · "}
              {t(($) => $.patrick_intro.role)}
            </span>
          </ItemTitle>
          <ItemDescription>{t(($) => $.patrick_intro.blurb)}</ItemDescription>
        </ItemContent>
      </Item>
    </div>
  );
}
