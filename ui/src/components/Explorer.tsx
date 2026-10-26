import { useVirtualizer } from "@tanstack/react-virtual";
import { useEffect, useMemo, useRef, useState } from "react";
import { registrationActions } from "../actions";
import { serverSummary, type ObjectKind } from "../api";
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
  /** Grey: a registration not connected, or a placeholder line */
  dim?: boolean;
  error?: boolean;
  busy?: boolean;
}

const ROW = 22;
const INDENT = 14;

export function Explorer() {
  const { registrations, servers, connecting, trees, objects, expanded, selection } = useStore();
  const { toggle, select, connect } = useStore.getState();
  const showMenu = useContextMenu((m) => m.show);
  const [focused, setFocused] = useState(false);
  const scroller = useRef<HTMLDivElement>(null);

  const rows = useMemo(() => {
    const rows: Row[] = [];
    for (const r of registrations) {
      const server = servers[r.id];
      const key = keys.registration(r.id);
      const open = !!server && !!expanded[key];
      rows.push({
        key,
        depth: 0,
        label: r.name,
        icon: "database",
        title: `${r.host}:${r.port} · ${r.database}${server ? `\n${serverSummary(server)}` : ""}`,
        expandable: !!server,
        expanded: open,
        selection: { id: r.id },
        dim: !server,
        busy: connecting[r.id],
      });
      if (!open) continue;
      const tree = trees[r.id];
      if (tree?.error) rows.push(placeholder(`${key}/e`, 1, tree.error, true));
      else if (!tree?.data) rows.push(placeholder(`${key}/l`, 1, t("Loading…")));
      for (const k of tree?.data ?? []) {
        const kindKey = keys.kind(r.id, k.kind);
        const kindOpen = k.count > 0 && !!expanded[kindKey];
        rows.push({
          key: kindKey,
          depth: 1,
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
        if (list?.error) rows.push(placeholder(`${kindKey}/e`, 2, list.error, true));
        else if (!list?.data) rows.push(placeholder(`${kindKey}/l`, 2, t("Loading…")));
        for (const o of list?.data ?? []) {
          rows.push({
            key: keys.object(r.id, k.kind, o.name),
            depth: 2,
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
    }
    return rows;
  }, [registrations, servers, connecting, trees, objects, expanded]);

  const selectedKey = selection ? keyOf(selection) : null;
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
    else if (row.expandable) open(row);
  };

  const moveTo = (index: number) => {
    const row = rows[Math.max(0, Math.min(rows.length - 1, index))];
    if (row?.selection) select(row.selection);
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
        else if (row && !row.selection?.kind) activate(row);
        break;
      case "ArrowLeft":
        if (row?.expanded) open(row, false);
        else if (row && row.depth > 0) {
          for (let i = selectedIndex - 1; i >= 0; i--) if (rows[i].depth < row.depth) return moveTo(i);
        }
        break;
      case "Enter":
        if (row) activate(row);
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
      </div>
    );
  }

  return (
    <div
      ref={scroller}
      role="tree"
      aria-label={t("Database Explorer")}
      tabIndex={0}
      className="scroll-thin h-full overflow-auto bg-panel outline-none"
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
              title={row.title}
              className={`absolute left-0 flex w-full items-center gap-1.5 pr-2 whitespace-nowrap ${
                selected ? (focused ? "bg-selected" : "bg-selected-inactive") : "hover:bg-hover"
              } ${row.error ? "text-danger" : row.dim ? "text-muted" : ""}`}
              style={{ top: v.start, height: ROW, paddingLeft: 4 + row.depth * INDENT }}
              onMouseDown={() => row.selection && select(row.selection)}
              onDoubleClick={() => activate(row)}
              onContextMenu={(e) => {
                e.preventDefault();
                if (!row.selection) return;
                select(row.selection);
                const id = row.selection.id;
                const items = row.selection.kind
                  ? [{ label: t("Refresh"), icon: "refresh", onSelect: () => void useStore.getState().refresh(id) }]
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
                  className={`shrink-0 ${row.dim && row.depth === 0 ? "text-faint" : (kindColor[row.icon] ?? "")} ${
                    row.busy ? "animate-pulse" : ""
                  }`}
                />
              )}
              <span className="truncate">{row.label}</span>
              {row.detail && <span className="truncate text-faint">{row.detail}</span>}
            </div>
          );
        })}
      </div>
    </div>
  );
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
  for (let i = from + step; i >= 0 && i < rows.length; i += step) if (rows[i].selection) return i;
  return from;
}
