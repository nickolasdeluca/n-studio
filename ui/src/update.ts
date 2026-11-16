// The app updating itself: asked once when it opens, and when the version
// in the status bar is clicked. A newer release is offered there; taking
// it ends what quitting ends, so it goes through the same questions
// (pending.ts) before the backend fetches it and starts it.
import { listen } from "@tauri-apps/api/event";
import { api, errorText } from "./api";
import { t } from "./i18n";
import { useStore } from "./store";

/** Asks whether there is a newer release; `true` when there is. Said
 *  nothing of when it cannot be asked, unless `aloud`: a machine that is
 *  offline is not told so at every start. */
export async function checkForUpdate(aloud = false): Promise<boolean> {
  try {
    const update = await api.updateCheck();
    useStore.setState({ update });
    return update !== null;
  } catch (e) {
    if (aloud) useStore.getState().setNotice(errorText(e));
    return false;
  }
}

/** Fetches the update and restarts into it. Comes back only when that
 *  failed, which the status bar then says. */
export async function installUpdate(): Promise<void> {
  useStore.setState({ updating: true });
  const stop = await listen<{ received: number; total: number | null }>("update-progress", ({ payload }) => {
    useStore.setState({ updating: payload.total ? Math.min(1, payload.received / payload.total) : true });
  }).catch(() => () => {});
  try {
    await api.updateInstall();
  } catch (e) {
    useStore.getState().setNotice(t("The update was not installed: {error}", { error: errorText(e) }));
  } finally {
    stop();
    useStore.setState({ updating: null });
  }
}
