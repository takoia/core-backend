// Minimal dependency-free i18n: a locale store + a reactive `t` translator,
// persisted to localStorage. UI strings live in en.ts / fr.ts, never hardcoded.

import { derived, writable } from "svelte/store";
import { en } from "./en";
import { fr } from "./fr";

export type Locale = "en" | "fr";
const dicts: Record<Locale, Record<string, string>> = { en, fr };

function initialLocale(): Locale {
  const stored = localStorage.getItem("locale");
  if (stored === "en" || stored === "fr") return stored;
  // French by default.
  return "fr";
}

export const locale = writable<Locale>(initialLocale());
locale.subscribe((l) => localStorage.setItem("locale", l));

export function setLocale(l: Locale) {
  locale.set(l);
}

/// Reactive translator: `$t('key')` or `$t('key', { n: 3 })`.
export const t = derived(locale, ($l) => {
  return (key: string, vars?: Record<string, string | number>): string => {
    let s = dicts[$l][key] ?? dicts.en[key] ?? key;
    if (vars) {
      for (const k of Object.keys(vars)) {
        s = s.replace(new RegExp(`\\{${k}\\}`, "g"), String(vars[k]));
      }
    }
    return s;
  };
});
