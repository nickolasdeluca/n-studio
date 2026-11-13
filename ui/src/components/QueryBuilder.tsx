// A query builder's tab: the tables on the left, each with its columns to
// tick; how they join and what each ticked column is there for in the
// middle; the statement that makes, as it will be run, at the bottom.
import type { IDockviewPanelProps } from "dockview-react";
import { useId, useState } from "react";
import type { Aggregate, JoinKind, QueryColumn, QueryJoin } from "../api";
import { useBuilders, type BuilderPanel } from "../builder";
import { openEditor } from "../dock";
import { t } from "../i18n";
import { Icon } from "../icons";
import { useStore } from "../store";
import { TextButton } from "./ObjectViewer";

const field = "h-6 min-w-0 rounded border border-line bg-panel px-1 text-fg outline-none focus:border-accent";
const heading = "flex h-7 shrink-0 items-center gap-2 border-b border-line bg-panel-2 px-2 font-semibold";

const joinKinds: [JoinKind, string][] = [
  ["inner", "JOIN"],
  ["left", "LEFT JOIN"],
  ["right", "RIGHT JOIN"],
  ["full", "FULL JOIN"],
];
const aggregates: Aggregate[] = ["count", "sum", "avg", "min", "max"];

function IconButton(props: { icon: string; title: string; disabled?: boolean; onClick(): void }) {
  return (
    <button
      type="button"
      title={props.title}
      aria-label={props.title}
      disabled={props.disabled}
      onClick={props.onClick}
      className="flex size-5 shrink-0 items-center justify-center rounded text-muted hover:bg-hover hover:text-fg disabled:text-faint disabled:hover:bg-transparent"
    >
      <Icon name={props.icon} size={12} />
    </button>
  );
}

/** A table of the query: what it is called there, and its columns */
function Source({ panel, name, alias }: { panel: BuilderPanel; name: string; alias: string }) {
  const builders = useBuilders.getState();
  const [typed, setTyped] = useState<string | null>(null);
  const table = panel.defs[name];
  const used = new Set(panel.spec.columns.filter((c) => c.table === alias).map((c) => c.column));
  const commit = () => {
    // A name already taken is not kept: the field goes back to what it was
    if (typed !== null) builders.renameTable(panel.id, alias, typed);
    setTyped(null);
  };
  return (
    <section className="border-b border-line">
      <header className="flex h-7 items-center gap-1 bg-panel-2 pr-1 pl-2">
        <Icon name="table" size={13} className="shrink-0 text-muted" />
        <input
          className="h-5 min-w-0 flex-1 rounded border border-transparent bg-transparent px-0.5 font-semibold text-fg outline-none hover:border-line focus:border-accent focus:bg-panel"
          value={typed ?? alias}
          title={alias === name ? t("What the query calls it") : t("{name}, called {alias} in the query", { name, alias })}
          spellCheck={false}
          onChange={(e) => setTyped(e.target.value)}
          onBlur={commit}
          onKeyDown={(e) => {
            if (e.key === "Enter") e.currentTarget.blur();
            if (e.key === "Escape") setTyped(null);
          }}
        />
        {alias !== name && <span className="shrink-0 truncate text-faint">{name}</span>}
        <IconButton icon="trash" title={t("Remove from the query")} onClick={() => builders.removeTable(panel.id, alias)} />
      </header>
      <ul className="py-0.5">
        {(table?.columns ?? []).map((c) => (
          <li key={c.name}>
            <label className="flex h-[21px] cursor-default items-center gap-1.5 px-2 hover:bg-hover">
              <input type="checkbox" checked={used.has(c.name)} onChange={() => builders.toggleColumn(panel.id, alias, c.name)} />
              <span className={`truncate ${c.primaryKey ? "font-semibold" : ""}`}>{c.name}</span>
              <span className="ml-auto shrink-0 pl-2 text-faint">{c.typeName}</span>
            </label>
          </li>
        ))}
      </ul>
    </section>
  );
}

function JoinRow({ panel, join, index }: { panel: BuilderPanel; join: QueryJoin; index: number }) {
  const builders = useBuilders.getState();
  const tables = panel.spec.tables;
  const columnsOf = (alias: string) => panel.defs[tables.find((x) => x.alias === alias)?.name ?? ""]?.columns ?? [];
  const patch = (p: Partial<QueryJoin>) => builders.patchJoin(panel.id, index, p);
  const pair = (at: number, side: 0 | 1, column: string) =>
    patch({ on: join.on.map((p, i): [string, string] => (i !== at ? p : side === 0 ? [column, p[1]] : [p[0], column])) });
  // The other table's first column: a side changed keeps no column of the old one
  const moved = (side: 0 | 1, alias: string) => {
    const first = columnsOf(alias)[0]?.name ?? "";
    patch({
      ...(side === 0 ? { left: alias } : { right: alias }),
      on: join.on.map((p): [string, string] => (side === 0 ? [first, p[1]] : [p[0], first])),
    });
  };
  const side = (which: 0 | 1, at: number) => {
    const alias = which === 0 ? join.left : join.right;
    return (
      <span className="flex min-w-0 items-center gap-1">
        {at === 0 ? (
          <select className={`${field} max-w-44`} value={alias} onChange={(e) => moved(which, e.target.value)} aria-label={t("Table")}>
            {tables.map((x) => (
              <option key={x.alias} value={x.alias}>
                {x.alias}
              </option>
            ))}
          </select>
        ) : (
          <span className="max-w-44 truncate px-1 text-faint">{alias}</span>
        )}
        <select className={`${field} max-w-44`} value={join.on[at][which]} onChange={(e) => pair(at, which, e.target.value)} aria-label={t("Column")}>
          {columnsOf(alias).map((c) => (
            <option key={c.name} value={c.name}>
              {c.name}
            </option>
          ))}
        </select>
      </span>
    );
  };
  return (
    <li className="flex flex-col gap-1 border-b border-line px-2 py-1">
      {join.on.map((_, at) => (
        <div key={at} className="flex flex-wrap items-center gap-1.5">
          {at === 0 ? (
            <select className={`${field} w-28`} value={join.kind} onChange={(e) => patch({ kind: e.target.value as JoinKind })} aria-label={t("Kind of join")}>
              {joinKinds.map(([kind, word]) => (
                <option key={kind} value={kind}>
                  {word}
                </option>
              ))}
            </select>
          ) : (
            <span className="w-28 pr-1 text-right text-faint">AND</span>
          )}
          {side(0, at)}
          <span className="text-muted">=</span>
          {side(1, at)}
          {at === 0 ? (
            <>
              <IconButton
                icon="plus"
                title={t("Another pair of columns")}
                onClick={() => patch({ on: [...join.on, [columnsOf(join.left)[0]?.name ?? "", columnsOf(join.right)[0]?.name ?? ""]] })}
              />
              <IconButton icon="trash" title={t("Remove the join")} onClick={() => builders.removeJoin(panel.id, index)} />
            </>
          ) : (
            <IconButton icon="trash" title={t("Remove the pair")} onClick={() => patch({ on: join.on.filter((_, i) => i !== at) })} />
          )}
        </div>
      ))}
    </li>
  );
}

const grid = "grid grid-cols-[2.2rem_minmax(9rem,1.4fr)_minmax(6rem,1fr)_5.5rem_7rem_minmax(8rem,1.4fr)_4.6rem] items-center gap-x-1.5 px-2";

function ColumnRow({ panel, column, index }: { panel: BuilderPanel; column: QueryColumn; index: number }) {
  const builders = useBuilders.getState();
  const patch = (p: Partial<QueryColumn>) => builders.patchColumn(panel.id, index, p);
  const name = panel.spec.tables.length > 1 ? `${column.table}.${column.column}` : column.column;
  return (
    <li className={`${grid} h-7 border-b border-line`}>
      <input type="checkbox" className="justify-self-center" checked={column.output} onChange={(e) => patch({ output: e.target.checked })} aria-label={t("In the result")} />
      <span className="truncate" title={name}>
        {name}
      </span>
      <input className={field} value={column.alias ?? ""} spellCheck={false} onChange={(e) => patch({ alias: e.target.value || null })} aria-label={t("Shown as")} />
      <select className={field} value={column.aggregate ?? ""} onChange={(e) => patch({ aggregate: (e.target.value || null) as Aggregate | null })} aria-label={t("Total")}>
        <option value="" />
        {aggregates.map((a) => (
          <option key={a} value={a}>
            {a.toUpperCase()}
          </option>
        ))}
      </select>
      <select className={field} value={column.sort ?? ""} onChange={(e) => patch({ sort: (e.target.value || null) as QueryColumn["sort"] })} aria-label={t("Sort")}>
        <option value="" />
        <option value="asc">{t("ascending")}</option>
        <option value="desc">{t("descending")}</option>
      </select>
      <input
        className={`${field} font-mono`}
        value={column.filter ?? ""}
        placeholder={index === 0 ? "> 10, LIKE 'A%', IS NULL" : ""}
        spellCheck={false}
        onChange={(e) => patch({ filter: e.target.value || null })}
        aria-label={t("Filter")}
      />
      <span className="flex justify-end">
        <IconButton icon="up" title={t("Move up")} disabled={index === 0} onClick={() => builders.moveColumn(panel.id, index, -1)} />
        <IconButton icon="down" title={t("Move down")} disabled={index === panel.spec.columns.length - 1} onClick={() => builders.moveColumn(panel.id, index, 1)} />
        <IconButton icon="trash" title={t("Remove")} onClick={() => builders.removeColumn(panel.id, index)} />
      </span>
    </li>
  );
}

export function QueryBuilder({ params }: IDockviewPanelProps<{ id: string }>) {
  const panel = useBuilders((s) => s.panels[params.id]);
  const connected = useStore((s) => (panel ? !!s.servers[panel.registration] : false));
  const [adding, setAdding] = useState("");
  const list = useId();

  // After a restart of the app there is no query behind the tab
  if (!panel) return <div className="h-full bg-panel px-2 py-1.5 text-muted">{t("This query is no longer kept.")}</div>;

  const builders = useBuilders.getState();
  const { spec } = panel;
  const add = () => {
    const name = adding.trim();
    if (!name || !connected) return;
    // As typed, or as the database spells it
    const known = panel.all.find((x) => x === name) ?? panel.all.find((x) => x.toUpperCase() === name.toUpperCase()) ?? name;
    setAdding("");
    void builders.addTable(panel.id, known);
  };
  // Tables the statement can only put side by side: every row with every row
  const loose = spec.tables.slice(1).filter((x, i) => {
    const before = spec.tables.slice(0, i + 1).map((b) => b.alias);
    return !spec.joins.some((j) => (j.right === x.alias && before.includes(j.left)) || (j.left === x.alias && before.includes(j.right)));
  });

  return (
    <div className="flex h-full flex-col bg-panel">
      <div className="flex min-h-8 shrink-0 flex-wrap items-center gap-x-3 gap-y-1 border-b border-line bg-panel-2 px-2 py-1">
        <input
          className={`${field} w-56`}
          placeholder={t("Add a table or view…")}
          list={list}
          value={adding}
          disabled={!connected}
          spellCheck={false}
          onChange={(e) => setAdding(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && add()}
        />
        <datalist id={list}>
          {panel.all.slice(0, 1000).map((name) => (
            <option key={name} value={name} />
          ))}
        </datalist>
        <label className="flex items-center gap-1.5 text-muted">
          <input type="checkbox" checked={spec.distinct} onChange={(e) => builders.change(panel.id, (s) => ({ ...s, distinct: e.target.checked }))} />
          {t("No repeated rows")}
        </label>
        <label className="flex items-center gap-1.5 text-muted">
          {t("Only the first")}
          <input
            className={`${field} w-16 tabular-nums`}
            inputMode="numeric"
            value={spec.first ?? ""}
            onChange={(e) => {
              const n = Number.parseInt(e.target.value.replace(/\D/g, "").slice(0, 9), 10);
              builders.change(panel.id, (s) => ({ ...s, first: Number.isFinite(n) && n > 0 ? n : null }));
            }}
          />
          {t("rows")}
        </label>
        {panel.loading && <span className="text-muted">{t("Loading…")}</span>}
        {!connected && <span className="text-muted">{t("Not connected.")}</span>}
        <div className="ml-auto">
          <TextButton icon="sql" label={t("To SQL Editor")} accent disabled={!panel.sql || !connected} onClick={() => openEditor(panel.registration, panel.sql)} />
        </div>
      </div>
      {panel.error && (
        <p className="shrink-0 border-b border-line px-2 py-1.5 whitespace-pre-wrap text-danger select-text" role="alert">
          {panel.error}
        </p>
      )}
      {spec.tables.length === 0 ? (
        <p className="px-2 py-1.5 text-muted">{t("Add the tables the query reads. Those related by a foreign key are joined by it.")}</p>
      ) : (
        <div className="flex min-h-0 flex-1">
          <div className="w-72 shrink-0 overflow-auto border-r border-line">
            {spec.tables.map((x) => (
              <Source key={x.alias} panel={panel} name={x.name} alias={x.alias} />
            ))}
          </div>
          <div className="flex min-w-0 flex-1 flex-col">
            <div className="min-h-0 flex-1 overflow-auto">
              {spec.tables.length > 1 && (
                <>
                  <div className={heading}>
                    {t("Joins")}
                    <IconButton icon="plus" title={t("Add a join")} onClick={() => builders.addJoin(panel.id)} />
                    {loose.length > 0 && (
                      <span className="font-normal text-danger">
                        {t("Joined to nothing before it, every row with every row: {tables}", { tables: loose.map((x) => x.alias).join(", ") })}
                      </span>
                    )}
                  </div>
                  <ul>
                    {spec.joins.map((join, i) => (
                      <JoinRow key={i} panel={panel} join={join} index={i} />
                    ))}
                  </ul>
                </>
              )}
              <div className={heading}>{t("Columns")}</div>
              {spec.columns.length === 0 ? (
                <p className="px-2 py-1.5 text-muted">{t("None ticked: the query shows them all.")}</p>
              ) : (
                <>
                  <div className={`${grid} h-6 border-b border-line text-muted`}>
                    <span className="justify-self-center">{t("Show")}</span>
                    <span>{t("Column")}</span>
                    <span>{t("Shown as")}</span>
                    <span>{t("Total")}</span>
                    <span>{t("Sort")}</span>
                    <span>{t("Filter")}</span>
                    <span />
                  </div>
                  <ul>
                    {spec.columns.map((column, i) => (
                      <ColumnRow key={i} panel={panel} column={column} index={i} />
                    ))}
                  </ul>
                </>
              )}
            </div>
            <pre className="h-2/5 max-h-80 min-h-24 shrink-0 overflow-auto border-t border-line bg-panel-2 px-2 py-1.5 font-mono text-fg select-text">
              {panel.sql}
            </pre>
          </div>
        </div>
      )}
    </div>
  );
}
