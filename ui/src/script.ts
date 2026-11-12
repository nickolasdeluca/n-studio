// Running a script in the backend: the call that starts it answers when it
// ends, and what fails on the way comes as events.
import { listen } from "@tauri-apps/api/event";
import { api, type ScriptEnd, type ScriptOptions, type ScriptProgress, type TxOptions } from "./api";

let runs = 0;
/** What each running script does with its progress, by run */
const progress = new Map<number, (event: ScriptProgress) => void>();
let subscribed: Promise<unknown> | null = null;

/** One listener for every script's progress, set up with the first script */
function listening(): Promise<unknown> {
  subscribed ??= listen<ScriptProgress>("script-progress", (event) => progress.get(event.payload.run)?.(event.payload)).catch(
    () => {
      // Outside Tauri there are no events: the script's end still tells all
    },
  );
  return subscribed;
}

/** A name for a script about to run, to follow and to cancel it by */
export const newRun = () => ++runs;

/** A transaction of its own, as an editor starts one */
export const defaultTx: TxOptions = { isolation: "read_committed", readOnly: false, lockWait: { mode: "wait" } };

/** Runs `text` on a registration, in `tx` if there is one. `onProgress`
 *  hears how far it is and what failed since it last heard; the answer has
 *  the totals and the failures no event carried. */
export async function runScript(
  run: number,
  registration: string,
  tx: number | null,
  txOptions: TxOptions,
  text: string,
  options: ScriptOptions,
  onProgress: (event: ScriptProgress) => void,
): Promise<ScriptEnd> {
  await listening();
  progress.set(run, onProgress);
  try {
    return await api.scriptRun(registration, run, tx, txOptions, text, options);
  } finally {
    progress.delete(run);
  }
}
