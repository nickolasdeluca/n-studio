// Two texts compared line by line, for the Script Executive to show what a
// script changes in an object: the longest common lines kept, the rest
// removed from the first text or added in the second.

export type DiffLine = { kind: "same" | "removed" | "added"; text: string };

/** Past this many lines on each side the texts are not aligned: all of the
 *  first is removed and all of the second added */
const MOST = 4000;

/** The lines of `from` and `to`, trailing spaces left out of the comparison */
export function lineDiff(from: string, to: string): DiffLine[] {
  const a = from.replace(/\r\n?/g, "\n").split("\n");
  const b = to.replace(/\r\n?/g, "\n").split("\n");
  if (a.length > MOST || b.length > MOST) {
    return [...a.map((text) => ({ kind: "removed" as const, text })), ...b.map((text) => ({ kind: "added" as const, text }))];
  }
  const key = (s: string) => s.trimEnd();
  // lcs[i][j]: the longest common lines of a[i..] and b[j..]
  const lcs = Array.from({ length: a.length + 1 }, () => new Uint16Array(b.length + 1));
  for (let i = a.length - 1; i >= 0; i--) {
    for (let j = b.length - 1; j >= 0; j--) {
      lcs[i][j] = key(a[i]) === key(b[j]) ? lcs[i + 1][j + 1] + 1 : Math.max(lcs[i + 1][j], lcs[i][j + 1]);
    }
  }
  const out: DiffLine[] = [];
  let i = 0;
  let j = 0;
  while (i < a.length && j < b.length) {
    if (key(a[i]) === key(b[j])) {
      out.push({ kind: "same", text: b[j] });
      i++;
      j++;
    } else if (lcs[i + 1][j] >= lcs[i][j + 1]) {
      out.push({ kind: "removed", text: a[i++] });
    } else {
      out.push({ kind: "added", text: b[j++] });
    }
  }
  while (i < a.length) out.push({ kind: "removed", text: a[i++] });
  while (j < b.length) out.push({ kind: "added", text: b[j++] });
  return out;
}
