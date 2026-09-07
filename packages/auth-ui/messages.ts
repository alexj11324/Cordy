import en from './locales/en.json';
import zhHans from './locales/zh-Hans.json';
export type AuthMessages = typeof en;
export const authMessages = { en, 'zh-Hans': zhHans };
export function messagesForLocale(locale: string): AuthMessages {
  return authMessages[locale as keyof typeof authMessages] ?? en;
}
