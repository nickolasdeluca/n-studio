// What the editors ran, as the backend keeps it (history.rs). Recording
// never stands in a statement's way: a history that cannot be written is
// a statement that still ran.
import { api } from "./api";

type Listener = (registration: string) => void;
const listeners = new Set<Listener>();

/** Tells when a registration's history has something new */
export function onHistory(listener: Listener): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export function historyChanged(registration: string): void {
  for (const listener of listeners) listener(registration);
}

/** Notes that `sql` ran on `registration`, and what it failed with */
export function record(registration: string, sql: string, elapsedMs: number, error: string | null): void {
  api.historyAdd(registration, sql, elapsedMs, error).then(
    (kept) => kept && historyChanged(registration),
    () => {},
  );
}
