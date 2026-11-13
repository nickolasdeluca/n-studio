// Run by `npm test`: Node's own runner, reading the TypeScript as it is.
import assert from "node:assert/strict";
import { test } from "node:test";
import { extensionOf, fromBase64, hexDump, isSvg, jsonOf, pictureOf, textBase64, textOf, xmlOf } from "./blob.ts";

const bytes = (...b) => new Uint8Array(b);
const ascii = (s) => new TextEncoder().encode(s);

test("a picture is told by its signature", () => {
  assert.equal(pictureOf(bytes(0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0, 0))?.mime, "image/png");
  assert.equal(pictureOf(bytes(0xff, 0xd8, 0xff, 0xe0))?.extension, "jpg");
  assert.equal(pictureOf(ascii("GIF89a....."))?.mime, "image/gif");
  assert.equal(pictureOf(ascii("RIFF\x10\x00\x00\x00WEBPVP8 "))?.mime, "image/webp");
  assert.equal(pictureOf(ascii("BM")), null, "two letters are not a bitmap");
  assert.equal(pictureOf(ascii("select 1")), null);
  assert.equal(pictureOf(bytes()), null);
});

test("an SVG is a picture kept as text", () => {
  assert.ok(isSvg('<svg xmlns="http://www.w3.org/2000/svg"></svg>'));
  assert.ok(isSvg('<?xml version="1.0"?>\n<!-- drawn -->\n<svg viewBox="0 0 1 1"/>'));
  assert.ok(!isSvg("<html><svg></svg></html>"));
});

test("files that are not pictures still get their extension", () => {
  assert.equal(extensionOf(ascii("%PDF-1.7")), "pdf");
  assert.equal(extensionOf(ascii("{\\rtf1\\ansi")), "rtf");
  assert.equal(extensionOf(ascii("plain")), null);
});

test("bytes that are text read as text, in the encoding they are in", () => {
  assert.deepEqual(textOf(ascii("ação\r\nfim")), { text: "ação\r\nfim", encoding: "UTF-8" });
  // "ação €" in Windows-1252
  assert.deepEqual(textOf(bytes(0x61, 0xe7, 0xe3, 0x6f, 0x20, 0x80)), { text: "ação €", encoding: "Windows-1252" });
  assert.equal(textOf(bytes(0x89, 0x50, 0x4e, 0x47, 0x00, 0x01)), null, "a zero byte is not text");
  assert.equal(textOf(bytes(1, 2, 3, 4, 5, 6, 7, 8)), null);
  assert.deepEqual(textOf(bytes()), { text: "", encoding: "UTF-8" });
});

test("a hex dump has sixteen bytes a line", () => {
  const dump = hexDump(ascii("0123456789abcdefXY\n"));
  assert.equal(
    dump,
    "00000000  30 31 32 33 34 35 36 37 38 39 61 62 63 64 65 66  0123456789abcdef\n" +
      "00000010  58 59 0A                                         XY.",
  );
});

test("JSON is indented when it is JSON", () => {
  assert.equal(jsonOf(' {"a":[1,2]} '), '{\n  "a": [\n    1,\n    2\n  ]\n}');
  assert.equal(jsonOf("12"), null, "a bare number is text");
  assert.equal(jsonOf("{not json"), null);
});

test("XML is indented a tag a line, text kept with its element", () => {
  assert.equal(
    xmlOf('<?xml version="1.0"?><nota><id>7</id><itens><item n="1"/><item>dois</item></itens><obs></obs></nota>'),
    '<?xml version="1.0"?>\n<nota>\n  <id>7</id>\n  <itens>\n    <item n="1"/>\n    <item>dois</item>\n  </itens>\n  <obs></obs>\n</nota>',
  );
  assert.equal(xmlOf("<a><b></a>"), null, "unbalanced");
  assert.equal(xmlOf("a < b"), null);
  assert.equal(xmlOf("<!-- only a comment -->"), null);
});

test("base64 goes both ways", () => {
  assert.deepEqual([...fromBase64("//79AA==")], [0xff, 0xfe, 0xfd, 0x00]);
  assert.equal(new TextDecoder().decode(fromBase64(textBase64("<svg>ação</svg>"))), "<svg>ação</svg>");
});
