import { useVirtualizer } from "@tanstack/react-virtual";
import { useDeferredValue, useMemo, useRef, useState } from "react";
import type { DbObject, TreeKind } from "../api";
import { t } from "../i18n";
import { Icon, kindColor } from "../icons";
import { keys, useObjects, useStore } from "../store";

const ROW = 22;

/** The objects of the kind selected in the explorer, beneath it */
export function ObjectList() {
  const selection = useStore((s) => s.selection);
  const registration = useStore((s) => s.registrations.find((r) => r.id === selection?.id));
  const server = useStore((s) => (selection ? s.servers[selection.id] : undefined));
  const tree = useStore((s) => (selection ? s.trees[selection.id] : undefined));
  const list = useObjects(selection?.id, selection?.kind);

  if (!selection || !registration) return <Empty text={t("Select a database in the explorer.")} />;
  if (!server) return <Empty text={t("{name} is not connected.", { name: registration.name })} />;

  if (!selection.kind) {
    return <KindSummary id={selection.id} kinds={tree?.data ?? []} loading={!tree?.data && !tree?.error} />;
  }
  const kind = tree?.data?.find((k) => k.kind === selection.kind);
  if (list?.error) return <Empty text={list.error} error />;
  return (
    <Objects
      key={`${selection.id}:${selection.kind}`}
      id={selection.id}
      kind={selection.kind}
      label={kind ? t(kind.label) : selection.kind}
      objects={list?.data}
      selected={selection.object}
    />
  );
}

function Objects(props: {
  id: string;
  kind: string;
  label: string;
  objects?: DbObject[];
  selected?: string;
}) {
  const { id, kind, label, objects, selected } = props;
  const [filter, setFilter] = useState("");
  const deferred = useDeferredValue(filter.trim().toUpperCase());
  const scroller = useRef<HTMLDivElement>(null);

  const rows = useMemo(
    () =>
      (objects ?? []).filter(
        (o) =>
          !deferred ||
          // Quoted names can be mixed-case
          o.name.toUpperCase().includes(deferred) ||
          o.parent?.toUpperCase().includes(deferred) ||
          o.description?.toUpperCase().includes(deferred),
      ),
    [objects, deferred],
  );
  const hasParent = rows.some((o) => o.parent);

  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scroller.current,
    estimateSize: () => ROW,
    overscan: 20,
  });

  const choose = (o: DbObject) => {
    const s = useStore.getState();
    s.select({ id, kind, object: o.name });
    // Reveal it in the tree, too
    s.toggle(keys.registration(id), true);
    s.toggle(keys.kind(id, kind), true);
  };

  const grid = hasParent ? "grid-cols-[minmax(10rem,2fr)_minmax(8rem,1.4fr)_3fr]" : "grid-cols-[minmax(10rem,2fr)_3fr]";

  return (
    <div className="flex h-full flex-col bg-panel">
      <div className="flex h-8 shrink-0 items-center gap-2 border-b border-line px-2">
        <Icon name={kind} className={kindColor[kind]} />
        <span className="font-medium whitespace-nowrap">{label}</span>
        <span className="whitespace-nowrap text-faint tabular-nums">
          {objects
            ? deferred
              ? `${rows.length.toLocaleString()} / ${objects.length.toLocaleString()}`
              : objects.length.toLocaleString()
            : t("Loading…")}
        </span>
        <label className="ml-auto flex h-6 w-48 min-w-16 shrink items-center gap-1.5 rounded border border-line bg-bg px-1.5 focus-within:border-accent">
          <Icon name="search" size={12} className="text-faint" />
          <input
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
            onKeyDown={(e) => e.key === "Escape" && setFilter("")}
            placeholder={t("Filter")}
            aria-label={t("Filter objects")}
            spellCheck={false}
            className="w-full bg-transparent outline-none placeholder:text-faint"
          />
        </label>
      </div>
      <div className={`grid ${grid} h-6 shrink-0 items-center gap-3 border-b border-line bg-panel-2 px-2 text-muted`}>
        <span>{t("Name")}</span>
        {hasParent && <span>{t("Table")}</span>}
        <span>{t("Description")}</span>
      </div>
      <div ref={scroller} className="scroll-thin min-h-0 flex-1 overflow-auto">
        <div style={{ height: virtualizer.getTotalSize(), position: "relative" }}>
          {virtualizer.getVirtualItems().map((v) => {
            const o = rows[v.index];
            return (
              <div
                key={o.name}
                className={`absolute left-0 grid w-full ${grid} items-center gap-3 px-2 whitespace-nowrap ${
                  o.name === selected ? "bg-selected" : "hover:bg-hover"
                }`}
                style={{ top: v.start, height: ROW }}
                onMouseDown={() => choose(o)}
                title={o.description ?? undefined}
              >
                <span className="truncate">{o.name}</span>
                {hasParent && <span className="truncate text-muted">{o.parent}</span>}
                <span className="truncate text-muted">{o.description?.split("\n")[0]}</span>
              </div>
            );
          })}
        </div>
        {objects && rows.length === 0 && (
          <p className="p-3 text-muted">{deferred ? t("Nothing matches the filter.") : t("None.")}</p>
        )}
      </div>
    </div>
  );
}

function KindSummary({ id, kinds, loading }: { id: string; kinds: TreeKind[]; loading: boolean }) {
  if (loading) return <Empty text={t("Loading…")} />;
  return (
    <div className="scroll-thin h-full overflow-auto bg-panel">
      <div className="grid h-6 grid-cols-[1fr_6rem] items-center border-b border-line bg-panel-2 px-2 text-muted">
        <span>{t("Object type")}</span>
        <span className="text-right">{t("Count")}</span>
      </div>
      {kinds.map((k) => (
        <div
          key={k.kind}
          className="grid h-[22px] grid-cols-[1fr_6rem] items-center px-2 hover:bg-hover"
          onDoubleClick={() => useStore.getState().select({ id, kind: k.kind })}
        >
          <span className="flex items-center gap-1.5">
            <Icon name={k.kind} className={kindColor[k.kind]} />
            {t(k.label)}
          </span>
          <span className={`text-right tabular-nums ${k.count ? "" : "text-faint"}`}>{k.count.toLocaleString()}</span>
        </div>
      ))}
    </div>
  );
}

function Empty({ text, error }: { text: string; error?: boolean }) {
  return (
    <div className={`grid h-full place-items-center bg-panel p-4 text-center ${error ? "text-danger" : "text-muted"}`}>
      {text}
    </div>
  );
}
