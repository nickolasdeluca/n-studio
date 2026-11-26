// What a user sets for this machine: the keys that run the app's commands,
// the sizes of its fonts and its language. Kept in the webview's storage,
// read before the first render, so the first screen is already in them.
import { create } from "zustand";

export type Language = "system" | "en" | "pt-BR";

export interface Fonts {
  /** The whole window, in percent */
  zoom: number;
  /** The SQL text, in pixels */
  editor: number;
  /** A result grid's cells, in pixels */
  grid: number;
}

export interface Prefs {
  /** The keys of the commands the user changed, by command id; the others
   *  have their defaults */
  keys: Record<string, string[]>;
  fonts: Fonts;
  language: Language;
}

export const FONT_DEFAULTS: Fonts = { zoom: 100, editor: 13, grid: 12 };
export const FONT_LIMITS: Record<keyof Fonts, [number, number, number]> = {
  zoom: [70, 200, 10],
  editor: [9, 28, 1],
  grid: [9, 24, 1],
};

const STORED = "n-studio.prefs.v1";

const clamp = (value: unknown, [min, max]: [number, number, number], fallback: number) =>
  typeof value === "number" && Number.isFinite(value) ? Math.max(min, Math.min(max, Math.round(value))) : fallback;

/** What was stored, as far as it makes sense: a key list of strings, a
 *  size in its range, a language there is */
export function readPrefs(text: string | null): Prefs {
  let raw: Partial<Record<keyof Prefs, unknown>> = {};
  try {
    const parsed: unknown = text ? JSON.parse(text) : {};
    if (parsed && typeof parsed === "object") raw = parsed as typeof raw;
  } catch {
    // A broken entry is as good as none
  }
  const keys: Record<string, string[]> = {};
  if (raw.keys && typeof raw.keys === "object") {
    for (const [id, list] of Object.entries(raw.keys as Record<string, unknown>)) {
      if (Array.isArray(list)) keys[id] = list.filter((k): k is string => typeof k === "string");
    }
  }
  const fonts = (raw.fonts && typeof raw.fonts === "object" ? raw.fonts : {}) as Partial<Record<keyof Fonts, unknown>>;
  const language = raw.language === "en" || raw.language === "pt-BR" ? raw.language : "system";
  return {
    keys,
    fonts: {
      zoom: clamp(fonts.zoom, FONT_LIMITS.zoom, FONT_DEFAULTS.zoom),
      editor: clamp(fonts.editor, FONT_LIMITS.editor, FONT_DEFAULTS.editor),
      grid: clamp(fonts.grid, FONT_LIMITS.grid, FONT_DEFAULTS.grid),
    },
    language,
  };
}

function load(): Prefs {
  // Outside a window (the tests) there is no storage to read
  if (typeof window === "undefined") return readPrefs(null);
  try {
    return readPrefs(localStorage.getItem(STORED));
  } catch {
    return readPrefs(null);
  }
}

interface PrefsState extends Prefs {
  /** The keys of a command; null gives it back its defaults */
  setKeys(id: string, keys: string[] | null): void;
  resetKeys(): void;
  setFont(which: keyof Fonts, size: number): void;
  setLanguage(language: Language): void;
}

export const usePrefs = create<PrefsState>((set, get) => {
  const save = (patch: Partial<Prefs>) => {
    set(patch);
    const { keys, fonts, language } = get();
    try {
      localStorage.setItem(STORED, JSON.stringify({ keys, fonts, language }));
    } catch {
      // Without storage they last until the app closes
    }
  };
  return {
    ...load(),
    setKeys: (id, keys) => {
      const next = { ...get().keys };
      if (keys === null) delete next[id];
      else next[id] = keys;
      save({ keys: next });
    },
    resetKeys: () => save({ keys: {} }),
    setFont: (which, size) =>
      save({ fonts: { ...get().fonts, [which]: clamp(size, FONT_LIMITS[which], FONT_DEFAULTS[which]) } }),
    setLanguage: (language) => save({ language }),
  };
});
