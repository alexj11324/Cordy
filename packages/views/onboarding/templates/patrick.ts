import type { PatrickOnboardingLanguage } from "@patchbay/core/onboarding";

export type PatrickContentLang = PatrickOnboardingLanguage;

interface LocalizedText {
  en: string;
  zh: string;
  ko: string;
  ja: string;
}

export interface PatrickOnboardingDefinition {
  title: string;
  language: PatrickContentLang;
}

/**
 * Patrick's name, description, avatar, permissions, and system instructions are
 * NOT here — they are server constants delivered by `POST /api/agents/patrick`.
 * Keeping them out of the client is what lets Patchbay update Patrick's prompt by
 * deploying, and stops a client from minting an agent that claims Patrick's
 * identity.
 *
 * The chat title stays client-side: it names a session this member is opening,
 * in the language they are currently using.
 */
const PATRICK_CHAT_TITLE: LocalizedText = {
  en: "Getting started with Patrick",
  zh: "和 Patrick 开始",
  ko: "Patrick과 시작하기",
  ja: "Patrick と始める",
};

export function getPatrickOnboarding(
  lang: PatrickContentLang,
): PatrickOnboardingDefinition {
  return {
    title: PATRICK_CHAT_TITLE[lang],
    language: lang,
  };
}
