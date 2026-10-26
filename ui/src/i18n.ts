// Every user-visible string goes through t(). English-only for now (PLAN §7):
// translating later is extracting a catalog, not touching every component.
export function t(text: string, vars?: Record<string, string | number>): string {
  if (!vars) return text;
  return text.replace(/\{(\w+)\}/g, (_, k: string) => String(vars[k] ?? `{${k}}`));
}
