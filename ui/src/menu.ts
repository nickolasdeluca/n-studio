// The texts of a Mac's menu bar, in the language in use: the bar is made in
// English, and made again with these once the window starts. Other systems
// have no menu bar, and the backend leaves them be.
import { api } from "./api";
import { locale, t, tx } from "./i18n";

export function sayMenu() {
  if (locale === "en") return;
  const app = "N Studio";
  api
    .menuTexts({
      about: t("About {app}", { app }),
      services: t("Services"),
      hide: t("Hide {app}", { app }),
      hideOthers: t("Hide Others"),
      quit: t("Quit {app}", { app }),
      file: t("File"),
      closeWindow: t("Close Window"),
      edit: t("Edit"),
      undo: t("Undo"),
      redo: t("Redo"),
      cut: t("Cut"),
      copy: t("Copy"),
      paste: t("Paste"),
      selectAll: t("Select All"),
      view: tx("menu", "View"),
      "view:explorer": t("Database Explorer"),
      "view:objects": t("Objects"),
      "view:agent": t("AI Agent"),
      "view:reset": t("Reset Layout"),
      fullscreen: t("Enter Full Screen"),
      window: t("Window"),
      minimize: t("Minimize"),
      zoom: t("Zoom"),
      help: t("Help"),
    })
    .catch(() => {});
}
