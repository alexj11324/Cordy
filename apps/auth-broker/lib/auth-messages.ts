"use client";
import { useEffect, useState } from "react";
import en from "../locales/en.json";
import ja from "../locales/ja.json";
import ko from "../locales/ko.json";
import zhHans from "../locales/zh-Hans.json";
type Messages = typeof en;
const locales: Record<string, Messages> = { en, ja, ko, zh: zhHans, "zh-hans": zhHans, "zh-cn": zhHans };
export function useAuthMessages(): Messages { const [messages, setMessages] = useState<Messages>(en); useEffect(() => { const language = navigator.language.toLowerCase(); setMessages(locales[language] ?? locales[language.split("-")[0] ?? ""] ?? en); }, []); return messages; }
