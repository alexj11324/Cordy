import en from './locales/en.json';
import ja from './locales/ja.json';
import ko from './locales/ko.json';
import zhHans from './locales/zh-Hans.json';
export type AuthMessages = typeof en;
export const authMessages = { en, ja, ko, 'zh-Hans': zhHans };
export function messagesForLocale(locale: string): AuthMessages {
  return authMessages[locale as keyof typeof authMessages] ?? en;
}
