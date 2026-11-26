// A Script Executive's queue: script files run from disk one after another,
// in the order of their names as migrations are named (V1__, V2__, V10__;
// 001_, 002_; 20240101_…) unless the user moved them. Pure: what the panel
// keeps and how it changes, without running anything.

export type QueueStatus = "pending" | "running" | "done" | "failed" | "stopped";

export interface QueueItem {
  path: string;
  status: QueueStatus;
  /** How its last run went: its counts, or why it did not run */
  note?: string;
  failed?: boolean;
}

export interface Queue {
  items: QueueItem[];
  /** Still in the order of the names: files added go to their place */
  byName: boolean;
  running: boolean;
  /** Stop before the next file */
  stopping: boolean;
}

export const emptyQueue: Queue = { items: [], byName: true, running: false, stopping: false };

/** The file's name, without its folder, on any system */
export function fileName(path: string): string {
  return path.slice(Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\")) + 1);
}

const collator = new Intl.Collator(undefined, { numeric: true, sensitivity: "base" });

/** By name with numbers as numbers (V2 before V10), then by folder */
export function byName(a: string, b: string): number {
  return collator.compare(fileName(a), fileName(b)) || collator.compare(a, b);
}

/** `paths` added, each once: in their place by name while the queue is in
 *  that order, after the others, by name among themselves, once it is not */
export function addFiles(queue: Queue, paths: string[]): Queue {
  const known = new Set(queue.items.map((i) => i.path));
  const added = [...new Set(paths)].filter((p) => !known.has(p)).sort(byName);
  if (added.length === 0) return queue;
  const fresh = added.map((path): QueueItem => ({ path, status: "pending" }));
  const items = queue.byName
    ? [...queue.items, ...fresh].sort((a, b) => byName(a.path, b.path))
    : [...queue.items, ...fresh];
  return { ...queue, items };
}

/** Back in the order of the names */
export function sortByName(queue: Queue): Queue {
  return { ...queue, items: [...queue.items].sort((a, b) => byName(a.path, b.path)), byName: true };
}

/** The file at `from` taken to `to`; the one running stays where it is */
export function move(queue: Queue, from: number, to: number): Queue {
  const { items } = queue;
  if (from === to || from < 0 || to < 0 || from >= items.length || to >= items.length) return queue;
  if (items[from].status === "running" || items[to].status === "running") return queue;
  const next = [...items];
  const [item] = next.splice(from, 1);
  next.splice(to, 0, item);
  return { ...queue, items: next, byName: next.every((it, i) => i === 0 || byName(next[i - 1].path, it.path) <= 0) };
}

/** The file left out of the queue, unless it is running */
export function remove(queue: Queue, path: string): Queue {
  const items = queue.items.filter((i) => i.path !== path || i.status === "running");
  return items.length === queue.items.length ? queue : { ...queue, items };
}

/** The files run already left out */
export function clearDone(queue: Queue): Queue {
  const items = queue.items.filter((i) => i.status !== "done");
  return items.length === queue.items.length ? queue : { ...queue, items };
}

/** Every file to be run again */
export function reset(queue: Queue): Queue {
  return { ...queue, items: queue.items.map((i) => (i.status === "running" ? i : { path: i.path, status: "pending" })) };
}

/** The next file to run: the first in the queue not run through. One that
 *  failed or was stopped is run again, after it was fixed. */
export function nextItem(queue: Queue): QueueItem | undefined {
  return queue.items.find((i) => i.status !== "done" && i.status !== "running");
}

/** The item for `path` changed by `p` */
export function patchItem(queue: Queue, path: string, p: Partial<QueueItem>): Queue {
  return { ...queue, items: queue.items.map((i) => (i.path === path ? { ...i, ...p } : i)) };
}
