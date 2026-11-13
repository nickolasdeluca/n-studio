// What a blob's bytes are, and the ways the viewer shows them. Pure: the
// component reads the cell and draws; `npm test` runs this as it is.

/** A picture the webview draws, by its MIME type and usual extension */
export interface Picture {
  mime: string;
  extension: string;
}

const starts = (bytes: Uint8Array, magic: number[], at = 0) =>
  bytes.length >= at + magic.length && magic.every((b, i) => bytes[at + i] === b);

/** The picture format the bytes begin as, told by their signature */
export function pictureOf(bytes: Uint8Array): Picture | null {
  if (starts(bytes, [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a])) return { mime: "image/png", extension: "png" };
  if (starts(bytes, [0xff, 0xd8, 0xff])) return { mime: "image/jpeg", extension: "jpg" };
  if (starts(bytes, [0x47, 0x49, 0x46, 0x38])) return { mime: "image/gif", extension: "gif" };
  if (starts(bytes, [0x42, 0x4d]) && bytes.length > 26) return { mime: "image/bmp", extension: "bmp" };
  if (starts(bytes, [0x52, 0x49, 0x46, 0x46]) && starts(bytes, [0x57, 0x45, 0x42, 0x50], 8))
    return { mime: "image/webp", extension: "webp" };
  if (starts(bytes, [0x00, 0x00, 0x01, 0x00])) return { mime: "image/x-icon", extension: "ico" };
  return null;
}

/** An SVG kept as text: it is drawn as a picture too */
export function isSvg(text: string): boolean {
  const head = text.slice(0, 1024).trimStart();
  return /^(<\?xml[^>]*\?>\s*)?(<!--[\s\S]*?-->\s*)*(<!DOCTYPE svg[^>]*>\s*)?<svg[\s>]/i.test(head);
}

/** The extension a file of these bytes is usually given, pictures aside */
export function extensionOf(bytes: Uint8Array): string | null {
  if (starts(bytes, [0x25, 0x50, 0x44, 0x46, 0x2d])) return "pdf";
  if (starts(bytes, [0x7b, 0x5c, 0x72, 0x74, 0x66])) return "rtf";
  if (starts(bytes, [0x50, 0x4b, 0x03, 0x04])) return "zip";
  if (starts(bytes, [0x1f, 0x8b])) return "gz";
  return null;
}

/** Windows-1252 where it differs from Latin-1: 0x80 to 0x9F */
const WIN1252 = "€\u0081‚ƒ„…†‡ˆ‰Š‹Œ\u008dŽ\u008f\u0090‘’“”•–—˜™š›œ\u009džŸ";

/**
 * The bytes as text, or `null` when they are not text. Legacy databases
 * keep their memos in subtype 0 blobs, which the server hands over as
 * bytes: UTF-8 when they are valid UTF-8, else Windows-1252, and not text
 * at all when control characters say they are something else.
 */
export function textOf(bytes: Uint8Array): { text: string; encoding: "UTF-8" | "Windows-1252" } | null {
  const sample = bytes.subarray(0, 4096);
  let control = 0;
  for (const b of sample) {
    if (b === 0) return null;
    if (b < 0x20 && b !== 0x09 && b !== 0x0a && b !== 0x0d && b !== 0x0c && b !== 0x1b) control++;
  }
  if (control > sample.length / 50) return null;
  try {
    return { text: new TextDecoder("utf-8", { fatal: true }).decode(bytes), encoding: "UTF-8" };
  } catch {
    return { text: win1252(bytes), encoding: "Windows-1252" };
  }
}

/** Every byte as the Windows-1252 character it is */
export function win1252(bytes: Uint8Array): string {
  const parts: string[] = [];
  for (let i = 0; i < bytes.length; i += 8192) {
    const chunk = bytes.subarray(i, i + 8192);
    let s = "";
    for (const b of chunk) s += b >= 0x80 && b <= 0x9f ? WIN1252[b - 0x80] : String.fromCharCode(b);
    parts.push(s);
  }
  return parts.join("");
}

/** Sixteen bytes a line: offset, hex, and the printable ones */
export function hexDump(bytes: Uint8Array): string {
  const lines: string[] = [];
  for (let i = 0; i < bytes.length; i += 16) {
    const chunk = bytes.subarray(i, i + 16);
    let hex = "";
    let ascii = "";
    for (const b of chunk) {
      hex += b.toString(16).toUpperCase().padStart(2, "0") + " ";
      ascii += b >= 0x20 && b < 0x7f ? String.fromCharCode(b) : ".";
    }
    lines.push(`${i.toString(16).toUpperCase().padStart(8, "0")}  ${hex.padEnd(48)} ${ascii}`);
  }
  return lines.join("\n");
}

/** The text indented as JSON, or `null` when it is not an object or array */
export function jsonOf(text: string): string | null {
  const trimmed = text.trim();
  if (!/^[[{]/.test(trimmed)) return null;
  try {
    return JSON.stringify(JSON.parse(trimmed), null, 2);
  } catch {
    return null;
  }
}

/**
 * The text indented as XML, or `null` when it is not XML. A tag a line;
 * an element holding only text stays on its line, so no text gains or
 * loses a space.
 */
export function xmlOf(text: string): string | null {
  const trimmed = text.trim();
  if (!/^<[?!a-zA-Z_]/.test(trimmed) || !trimmed.endsWith(">")) return null;
  const tokens = trimmed.match(/<!--[\s\S]*?-->|<!\[CDATA\[[\s\S]*?\]\]>|<[^>]+>|[^<]+/g);
  if (!tokens) return null;
  const lines: string[] = [];
  let depth = 0;
  let opened = 0;
  for (let i = 0; i < tokens.length; i++) {
    const token = tokens[i];
    if (!token.startsWith("<")) {
      if (token.trim()) lines.push("  ".repeat(depth) + token.trim());
      continue;
    }
    const closing = token.startsWith("</");
    const alone = /^<[?!]/.test(token) || token.endsWith("/>");
    if (closing) {
      depth--;
      if (depth < 0) return null;
      lines.push("  ".repeat(depth) + token);
    } else if (alone) {
      lines.push("  ".repeat(depth) + token);
    } else {
      opened++;
      // <a>text</a> on one line
      const next = tokens[i + 1];
      const after = tokens[i + 2];
      if (next !== undefined && !next.startsWith("<") && after?.startsWith("</")) {
        lines.push("  ".repeat(depth) + token + next + after);
        i += 2;
      } else if (next?.startsWith("</")) {
        lines.push("  ".repeat(depth) + token + next);
        i += 1;
      } else {
        lines.push("  ".repeat(depth) + token);
        depth++;
      }
    }
  }
  return depth === 0 && opened > 0 ? lines.join("\n") : null;
}

/** Bytes from the base64 the backend sends them in */
export function fromBase64(data: string): Uint8Array {
  const binary = atob(data);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
  return bytes;
}

/** Text as base64, for a `data:` URL */
export function textBase64(text: string): string {
  const bytes = new TextEncoder().encode(text);
  let binary = "";
  for (let i = 0; i < bytes.length; i += 8192) binary += String.fromCharCode(...bytes.subarray(i, i + 8192));
  return btoa(binary);
}
