// A database's catalog changed under the app's own hands: a script ran DDL,
// an object was compiled. Whoever shows the catalog listens here, so those
// who change it need not know who that is.
type Listener = (registration: string) => void;

const listeners = new Set<Listener>();

export function onMetadataChanged(listener: Listener): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export function metadataChanged(registration: string): void {
  for (const listener of listeners) listener(registration);
}
