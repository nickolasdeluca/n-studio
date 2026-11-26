// The font sizes of the preferences, put to use: the window's zoom, the SQL
// text's size and the grids' (which `ResultGrid` reads itself).
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { FONT_LIMITS, usePrefs, type Fonts } from "./prefs";

function apply(fonts: Fonts, before?: Fonts) {
  document.documentElement.style.setProperty("--editor-font-size", `${fonts.editor}px`);
  if (fonts.zoom !== before?.zoom) {
    // The webview's own zoom: everything, the menus and dialogs too, at
    // the size the system draws it, sharp
    getCurrentWebview()
      .setZoom(fonts.zoom / 100)
      .catch(() => {});
  }
  // The editors measure their lines again on a resize
  if (before && fonts.editor !== before.editor) window.dispatchEvent(new Event("resize"));
}

/** Puts the sizes to use now, and whenever they change */
export function applyFonts() {
  apply(usePrefs.getState().fonts);
  usePrefs.subscribe((now, was) => {
    if (now.fonts !== was.fonts) apply(now.fonts, was.fonts);
  });
}

/** The window's zoom a step bigger (1), smaller (-1) or back to 100% (0) */
export function zoom(by: -1 | 0 | 1) {
  const { fonts, setFont } = usePrefs.getState();
  const [, , step] = FONT_LIMITS.zoom;
  setFont("zoom", by === 0 ? 100 : fonts.zoom + by * step);
}
