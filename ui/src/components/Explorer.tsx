import { useVirtualizer } from "@tanstack/react-virtual";
import { useEffect, useMemo, useRef, useState } from "react";
import { folderActions, kindActions, objectActions, registrationActions } from "../actions";
import { openObject } from "../dock";
import { errorText, serverSummary, type ObjectKind, type RegistrationView } from "../api";
import { t } from "../i18n";
import { Icon, kindColor } from "../icons";
import { keys, useStore, type NodeKey, type Selection } from "../store";
import { useContextMenu } from "./ContextMenu";

/** One visible line of the tree */
interface Row {
  key: NodeKey;
  depth: number;
  label: string;
  icon?: string;
  /** Dimmed text after the label: a count, a parent table */
  detail?: string;
  title?: string;
  expandable: boolean;
  expanded: boolean;
  selection?: Selection;
  /** A folder of registrations, by its path: the keyboard stops on it, and
   *  nothing is selected while it does */
  folder?: string[];
  /** Grey: a registration not connected, or a placeholder line */
  dim?: boolean;
  error?: boolean;
  busy?: boolean;
}

const ROW = 22;
const INDENT = 14;

/** What is being dragged: a database, or a folder with all that is in it */
type Source = { type: "registration"; id: string } | { type: "folder"; path: string[] };

/** Where a drag would land, and the row that shows it */
type Target =
  /** Last in a folder; no path is the top, and no row the whole tree */
  | { type: "into"; path: string[]; row: NodeKey | null }
  /** Before or after a database, in its folder */
  | { type: "beside"; id: string; after: boolean; row: NodeKey };

interface Drag {
  source: Source;
  label: string;
  x: number;
  y: number;
  target: Target | null;
}

/** A press becomes a drag once it has moved this far */
const DRAG_AFTER = 5;

export function Explorer() {
  const { registrations, servers, connecting, trees, objects, expanded, selection } = useStore();
  const { toggle, select, connect } = useStore.getState();
  const showMenu = useContextMenu((m) => m.show);
  const [focused, setFocused] = useState(false);
  // The folder the keyboard is on: a folder is not a selection, which is
  // a database or something of it
  const [onFolder, setOnFolder] = useState<NodeKey | null>(null);
  useEffect(() => {
    if (selection) setOnFolder(null);
  }, [selection]);
  const scroller = useRef<HTMLDivElement>(null);

  const rows = useMemo(() => {
    const rows: Row[] = [];
    const add = (r: RegistrationView, depth: number) => {
      const server = servers[r.id];
      const key = keys.registration(r.id);
      const open = !!server && !!expanded[key];
      rows.push({
        key,
        depth,
        label: r.name,
        icon: "database",
        title: `${r.host}:${r.port} · ${r.database}${server ? `\n${serverSummary(server)}` : ""}`,
        expandable: !!server,
        expanded: open,
        selection: { id: r.id },
        dim: !server,
        busy: connecting[r.id],
      });
      if (!open) return;
      const tree = trees[r.id];
      if (tree?.error) rows.push(placeholder(`${key}/e`, depth + 1, tree.error, true));
      else if (!tree?.data) rows.push(placeholder(`${key}/l`, depth + 1, t("Loading…")));
      for (const k of tree?.data ?? []) {
        const kindKey = keys.kind(r.id, k.kind);
        const kindOpen = k.count > 0 && !!expanded[kindKey];
        rows.push({
          key: kindKey,
          depth: depth + 1,
          label: t(k.label),
          detail: `(${k.count.toLocaleString()})`,
          icon: k.kind,
          expandable: k.count > 0,
          expanded: kindOpen,
          selection: { id: r.id, kind: k.kind },
          dim: k.count === 0,
        });
        if (!kindOpen) continue;
        const list = objects[`${r.id}:${k.kind}`];
        if (list?.error) rows.push(placeholder(`${kindKey}/e`, depth + 2, list.error, true));
        else if (!list?.data) rows.push(placeholder(`${kindKey}/l`, depth + 2, t("Loading…")));
        for (const o of list?.data ?? []) {
          rows.push({
            key: keys.object(r.id, k.kind, o.name),
            depth: depth + 2,
            label: o.name,
            detail: o.parent ?? undefined,
            title: o.description ?? undefined,
            icon: k.kind,
            expandable: false,
            expanded: false,
            selection: { id: r.id, kind: k.kind, object: o.name },
          });
        }
      }
    };
    const list = (folder: Folder) => {
      for (const item of folder.items) {
        if (!("items" in item)) {
          add(item, folder.path.length);
          continue;
        }
        const key = keys.folder(item.path);
        // Open until it is closed: a folder hides what the explorer is for
        const open = expanded[key] !== false;
        rows.push({
          key,
          depth: folder.path.length,
          label: item.path[item.path.length - 1],
          detail: open ? undefined : `(${count(item)})`,
          icon: "folder",
          expandable: true,
          expanded: open,
          folder: item.path,
        });
        if (open) list(item);
      }
    };
    list(folders(registrations));
    return rows;
  }, [registrations, servers, connecting, trees, objects, expanded]);

  const selectedKey = selection ? keyOf(selection) : onFolder;

  // Dragging a database or a folder: into a folder, beside a database, or
  // to the empty space under the tree, which is the top. Mouse events and
  // not the browser's drag and drop, which a Tauri window on Windows keeps
  // for files dropped from outside.
  const [drag, setDrag] = useState<Drag | null>(null);
  const latest = useRef(rows);
  latest.current = rows;

  const targetAt = (source: Source, x: number, y: number): Target | null => {
    const under = document.elementFromPoint(x, y);
    if (!under || !scroller.current?.contains(under)) return null;
    const element = under.closest<HTMLElement>("[data-index]");
    const row = element ? latest.current[Number(element.dataset.index)] : undefined;
    // A folder cannot go into itself, or into one inside it
    const into = (path: string[], key: NodeKey | null): Target | null =>
      source.type === "folder" && source.path.every((name, i) => path[i] === name) ? null : { type: "into", path, row: key };
    if (!element || !row) return into([], null);
    if (row.folder) return into(row.folder, row.key);
    const id = row.selection?.id;
    const registration = id ? useStore.getState().registrations.find((r) => r.id === id) : undefined;
    if (!registration) return null;
    // A folder has no place among databases: it goes to the folder they are in
    if (source.type === "folder") {
      return into(registration.folder, registration.folder.length ? keys.folder(registration.folder) : null);
    }
    if (registration.id === source.id) return null;
    // Over what a connected database has open, it is after that database
    const key = keys.registration(registration.id);
    const box = element.getBoundingClientRect();
    return { type: "beside", id: registration.id, after: row.key !== key || y > box.top + box.height / 2, row: key };
  };

  const drop = ({ source, target }: Drag) => {
    if (!target) return;
    const s = useStore.getState();
    if (source.type === "folder") {
      const to = [...(target.type === "into" ? target.path : []), source.path[source.path.length - 1]];
      void s.moveFolder(source.path, to).catch((e) => s.setNotice(errorText(e)));
    } else if (target.type === "into") {
      void s.placeRegistration(source.id, target.path);
    } else {
      const folder = s.registrations.find((r) => r.id === target.id)?.folder ?? [];
      void s.placeRegistration(source.id, folder, target.id, target.after);
    }
  };

  const press = (row: Row, e: React.MouseEvent) => {
    const source: Source | null = row.folder
      ? { type: "folder", path: row.folder }
      : row.selection && !row.selection.kind
        ? { type: "registration", id: row.selection.id }
        : null;
    if (!source || e.button !== 0) return;
    const from = { x: e.clientX, y: e.clientY };
    let current: Drag | null = null;
    const move = (ev: MouseEvent) => {
      if (!current && Math.hypot(ev.clientX - from.x, ev.clientY - from.y) < DRAG_AFTER) return;
      current = { source, label: row.label, x: ev.clientX, y: ev.clientY, target: targetAt(source, ev.clientX, ev.clientY) };
      setDrag(current);
      // Near an edge, the tree scrolls to what is past it
      const box = scroller.current?.getBoundingClientRect();
      if (box && ev.clientY < box.top + ROW) scroller.current?.scrollBy(0, -ROW);
      else if (box && ev.clientY > box.bottom - ROW) scroller.current?.scrollBy(0, ROW);
    };
    const end = () => {
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", up);
      window.removeEventListener("keydown", key, true);
      setDrag(null);
    };
    const up = () => {
      const done = current;
      end();
      if (done) drop(done);
    };
    const key = (ev: KeyboardEvent) => {
      if (ev.key !== "Escape") return;
      ev.stopPropagation();
      end();
    };
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
    window.addEventListener("keydown", key, true);
  };

  const dragged = drag ? (drag.source.type === "folder" ? keys.folder(drag.source.path) : keys.registration(drag.source.id)) : null;
  const target = drag?.target ?? null;
  const selectedIndex = rows.findIndex((r) => r.key === selectedKey);

  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scroller.current,
    estimateSize: () => ROW,
    overscan: 20,
  });

  // Keep the selection in view, wherever it came from (the object list too)
  useEffect(() => {
    if (selectedIndex >= 0) virtualizer.scrollToIndex(selectedIndex, { align: "auto" });
  }, [selectedIndex, virtualizer]);

  const open = (row: Row, state?: boolean) => {
    const next = state ?? !row.expanded;
    toggle(row.key, next);
    // A kind's objects load when it opens
    if (next && row.selection?.kind && !row.selection.object) {
      void useStore.getState().loadObjects(row.selection.id, row.selection.kind as ObjectKind);
    }
  };

  const activate = (row: Row) => {
    const sel = row.selection;
    if (!sel) return;
    if (!sel.kind && !servers[sel.id]) void connect(sel.id);
    else if (sel.kind && sel.object) openObject(sel.id, sel.kind, sel.object);
    else if (row.expandable) open(row);
  };

  const pick = (row: Row) => {
    if (row.selection) select(row.selection);
    else if (row.folder) {
      select(null);
      setOnFolder(row.key);
    }
  };

  const moveTo = (index: number) => {
    const row = rows[Math.max(0, Math.min(rows.length - 1, index))];
    if (row) pick(row);
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    const row = rows[selectedIndex];
    switch (e.key) {
      case "ArrowDown":
        moveTo(nextSelectable(rows, selectedIndex, 1));
        break;
      case "ArrowUp":
        moveTo(nextSelectable(rows, selectedIndex, -1));
        break;
      case "Home":
        moveTo(0);
        break;
      case "End":
        moveTo(rows.length - 1);
        break;
      case "ArrowRight":
        if (row?.expandable && !row.expanded) open(row, true);
        else if (row?.expanded) moveTo(selectedIndex + 1);
        else if (row && !row.folder && !row.selection?.kind) activate(row);
        break;
      case "ArrowLeft":
        if (row?.expanded) open(row, false);
        else if (row && row.depth > 0) {
          for (let i = selectedIndex - 1; i >= 0; i--) if (rows[i].depth < row.depth) return moveTo(i);
        }
        break;
      case "Enter":
        if (row?.folder) open(row);
        else if (row) activate(row);
        break;
      default:
        return;
    }
    e.preventDefault();
  };

  if (registrations.length === 0) {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-3 p-6 text-center text-muted">
        <Icon name="database" size={28} className="text-faint" />
        <p>{t("No databases registered yet.")}</p>
        <button
          className="rounded border border-line bg-panel-2 px-3 py-1 text-fg hover:bg-hover"
          onClick={() => useStore.getState().openDialog({ type: "registration", registration: null })}
        >
          {t("Register Database…")}
        </button>
        <button
          className="text-accent hover:underline"
          onClick={() => useStore.getState().openDialog({ type: "ibexpert" })}
        >
          {t("Import from IBExpert…")}
        </button>
      </div>
    );
  }

  return (
    <div
      ref={scroller}
      role="tree"
      aria-label={t("Database Explorer")}
      tabIndex={0}
      className={`scroll-thin h-full overflow-auto bg-panel outline-none ${drag ? "cursor-grabbing select-none" : ""} ${
        target?.type === "into" && !target.row ? "shadow-[inset_0_0_0_1px_var(--accent)]" : ""
      }`}
      onKeyDown={onKeyDown}
      onFocus={() => setFocused(true)}
      onBlur={() => setFocused(false)}
    >
      <div style={{ height: virtualizer.getTotalSize(), position: "relative" }}>
        {virtualizer.getVirtualItems().map((v) => {
          const row = rows[v.index];
          const selected = row.key === selectedKey;
          return (
            <div
              key={row.key}
              role="treeitem"
              aria-level={row.depth + 1}
              aria-expanded={row.expandable ? row.expanded : undefined}
              aria-selected={selected}
              data-index={v.index}
              title={drag ? undefined : row.title}
              className={`absolute left-0 flex w-full items-center gap-1.5 pr-2 whitespace-nowrap ${
                target?.type === "into" && target.row === row.key
                  ? "bg-hover shadow-[inset_0_0_0_1px_var(--accent)]"
                  : selected
                    ? focused
                      ? "bg-selected"
                      : "bg-selected-inactive"
                    : drag
                      ? ""
                      : "hover:bg-hover"
              } ${row.error ? "text-danger" : row.dim ? "text-muted" : ""} ${row.key === dragged ? "opacity-50" : ""}`}
              style={{ top: v.start, height: ROW, paddingLeft: 4 + row.depth * INDENT }}
              onMouseDown={(e) => {
                pick(row);
                press(row, e);
              }}
              onDoubleClick={() => (row.folder ? open(row) : activate(row))}
              onContextMenu={(e) => {
                e.preventDefault();
                if (row.folder) showMenu(e.clientX, e.clientY, folderActions(row.folder));
                if (!row.selection) return;
                select(row.selection);
                const { id, kind, object } = row.selection;
                const refresh = { label: t("Refresh"), icon: "refresh", onSelect: () => void useStore.getState().refresh(id) };
                const items =
                  kind && object
                    ? [...objectActions(id, [{ kind, name: object }]), "separator" as const, refresh]
                    : kind
                      ? [...kindActions(id, kind, trees[id]?.data?.find((k) => k.kind === kind)?.singular ?? kind), "separator" as const, refresh]
                      : registrationActions(id);
                showMenu(e.clientX, e.clientY, items);
              }}
            >
              <span
                className={`grid w-3.5 shrink-0 place-items-center text-faint ${row.expandable ? "" : "invisible"}`}
                onMouseDown={(e) => {
                  e.stopPropagation();
                  open(row);
                }}
              >
                <Icon name="chevron" size={11} className={`transition-transform ${row.expanded ? "rotate-90" : ""}`} />
              </span>
              {row.icon && (
                <Icon
                  name={row.icon}
                  className={`shrink-0 ${row.folder || (row.dim && row.icon === "database") ? "text-faint" : (kindColor[row.icon] ?? "")} ${
                    row.busy ? "animate-pulse" : ""
                  }`}
                />
              )}
              <span className="truncate">{row.label}</span>
              {row.detail && <span className="truncate text-faint">{row.detail}</span>}
              {target?.type === "beside" && target.row === row.key && (
                <span
                  className="pointer-events-none absolute right-1 h-0.5 rounded bg-accent"
                  style={{ left: 4 + row.depth * INDENT, [target.after ? "bottom" : "top"]: -1 }}
                />
              )}
            </div>
          );
        })}
      </div>
      {drag && (
        <div
          className="pointer-events-none fixed z-[900] flex items-center gap-1.5 rounded border border-line bg-panel-2 px-2 py-0.5 shadow-[var(--shadow)]"
          style={{ left: drag.x + 12, top: drag.y + 10 }}
        >
          <Icon name={drag.source.type === "folder" ? "folder" : "database"} className="text-faint" />
          {drag.label}
        </div>
      )}
    </div>
  );
}

/** A folder of the explorer: registrations, and folders inside it */
interface Folder {
  path: string[];
  items: (Folder | RegistrationView)[];
}

/** The registrations as a tree. A folder comes where its first registration is. */
function folders(registrations: RegistrationView[]): Folder {
  const root: Folder = { path: [], items: [] };
  const byKey = new Map<string, Folder>();
  for (const r of registrations) {
    let folder = root;
    for (let depth = 1; depth <= r.folder.length; depth++) {
      const path = r.folder.slice(0, depth);
      const key = keys.folder(path);
      let inner = byKey.get(key);
      if (!inner) {
        inner = { path, items: [] };
        byKey.set(key, inner);
        folder.items.push(inner);
      }
      folder = inner;
    }
    folder.items.push(r);
  }
  return root;
}

/** The registrations under a folder, however deep */
function count(folder: Folder): number {
  return folder.items.reduce((n, item) => n + ("items" in item ? count(item) : 1), 0);
}

function placeholder(key: string, depth: number, label: string, error = false): Row {
  return { key, depth, label, expandable: false, expanded: false, dim: true, error };
}

function keyOf(s: Selection): NodeKey {
  if (s.object && s.kind) return keys.object(s.id, s.kind, s.object);
  if (s.kind) return keys.kind(s.id, s.kind);
  return keys.registration(s.id);
}

function nextSelectable(rows: Row[], from: number, step: 1 | -1): number {
  for (let i = from + step; i >= 0 && i < rows.length; i += step) if (rows[i].selection || rows[i].folder) return i;
  return from;
}
