// SPDX-License-Identifier: GPL-3.0-or-later
import i18next, { type ParseKeys } from "i18next";
import { initReactI18next } from "react-i18next";
import en from "./locales/en.json";

/** Every user-visible string lives in a catalog namespace; JSX never carries raw text. */
export const defaultNS = "editor";
export const resources = { en: { editor: en } } as const;

declare module "i18next" {
  interface CustomTypeOptions {
    defaultNS: typeof defaultNS;
    resources: (typeof resources)["en"];
  }
}

/** A key that exists in the `en` catalog (checked by the compiler). */
export type TKey = ParseKeys<typeof defaultNS>;

export const SUPPORTED_LANGUAGES = ["en"] as const;
export type Language = (typeof SUPPORTED_LANGUAGES)[number];

/**
 * The best supported language for the OS/browser's preferred languages (`navigator.languages`):
 * the first whose primary subtag we have a catalog for, else English. Only `en` ships today
 * (translations arrive in P7), so this is the seam, not a behaviour change.
 */
export function detectLanguage(preferred: readonly string[]): Language {
  for (const tag of preferred) {
    const primary = tag.toLowerCase().split("-")[0];
    const found = SUPPORTED_LANGUAGES.find((lang) => lang === primary);
    if (found !== undefined) return found;
  }
  return "en";
}

/** A fresh, synchronously initialised instance (tests use one per case). */
export function createI18n(lng: Language = "en") {
  const instance = i18next.createInstance();
  instance.use(initReactI18next).init({
    resources,
    lng,
    fallbackLng: "en",
    defaultNS,
    ns: [defaultNS],
    interpolation: { escapeValue: false },
    initAsync: false,
  });
  return instance;
}

export const i18n = createI18n(
  detectLanguage(typeof navigator === "undefined" ? [] : (navigator.languages ?? [])),
);
