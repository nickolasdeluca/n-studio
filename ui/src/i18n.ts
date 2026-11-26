// Every user-visible string goes through t(): its English text is the key,
// and the language in use says it from its catalog (`locales/`). A text a
// catalog does not have is shown in English.
//
// The language is chosen once, when the app starts: the preferences' one,
// or the system's when they leave it to the system. Changing it takes the
// next start, so nothing on screen is left in the language before.
import { ptBR } from "./locales/pt-BR";
import { localeOf, systemLocale, usePrefs, type Locale } from "./prefs";

const catalogs: Record<Locale, Record<string, string> | null> = { en: null, "pt-BR": ptBR };

const systemLanguages = () => (navigator.languages?.length ? navigator.languages : [navigator.language]);

/** The language in use since the app started */
export const locale: Locale = localeOf(usePrefs.getState().language, systemLanguages());

/** The language "the system's" stands for */
export const systemDefault = (): Locale => systemLocale(systemLanguages());

document.documentElement.lang = locale;

const catalog = catalogs[locale];

type Vars = Record<string, string | number>;

const fill = (said: string, vars?: Vars) =>
  vars ? said.replace(/\{(\w+)\}/g, (_, k: string) => String(vars[k] ?? `{${k}}`)) : said;

export function t(text: string, vars?: Vars): string {
  return fill(catalog?.[text] ?? text, vars);
}

/** A text that means another thing elsewhere: "View" the menu is not "View"
 *  the kind of object. Its catalog entry is "text [context]". */
export function tx(context: string, text: string, vars?: Vars): string {
  return fill(catalog?.[`${text} [${context}]`] ?? catalog?.[text] ?? text, vars);
}
