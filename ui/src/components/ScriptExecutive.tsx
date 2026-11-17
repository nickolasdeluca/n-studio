// The Script Executive: a script as work in itself, run on a connection of
// its own, all of it or only the statements checked. The script on one
// tab, its statements on another, the options on a third; what failed
// below.
import { Compartment, EditorState, Prec, RangeSet, StateEffect, StateField } from "@codemirror/state";
import { Decoration, EditorView, GutterMarker, gutterLineClass, keymap, type DecorationSet } from "@codemirror/view";
import type { IDockviewPanelProps } from "dockview-react";
import { useCallback, useEffect, useRef, useState } from "react";
import type { ExecutiveOptions, ScriptBlock } from "../api";
import {
  endExecutive,
  executiveEdited,
  openExecutiveFile,
  patchExecutive,
  runExecutive,
  saveExecutive,
  stopExecutive,
  texts,
  useExecutives,
  type Executive,
  type ExecutiveTab,
} from "../executive";
import { t } from "../i18n";
import { Icon } from "../icons";
import { sqlExtensions } from "../sqlView";
import { TextButton } from "./ObjectViewer";

/** Editor states by panel, so a panel that remounts keeps its text and undo */
const saved = new Map<string, EditorState>();
/** What the keys call, by panel: the mounted panel's callbacks */
const keys = new Map<string, { run(): void; save(): void }>();

export function forgetExecutive(id: string) {
  saved.delete(id);
  keys.delete(id);
}

// ── Marks: the statement running, those that failed ──────────────────────

interface Marks {
  running: { from: number; to: number } | null;
  failed: { from: number; to: number }[];
}

const setMarks = StateEffect.define<Marks>();

class LineClass extends GutterMarker {
  constructor(readonly elementClass: string) {
    super();
  }
}
const runningLine = new LineClass("cm-exe-running");
const failedLine = new LineClass("cm-exe-failed");

function build(state: EditorState, marks: Marks): { text: DecorationSet; gutter: RangeSet<GutterMarker> } {
  const len = state.doc.length;
  const text = [];
  const gutter = [];
  const clip = (r: { from: number; to: number }) => ({ from: Math.min(r.from, len), to: Math.min(r.to, len) });
  for (const r of marks.failed.map(clip)) {
    if (r.to > r.from) text.push(Decoration.mark({ class: "cm-error-statement" }).range(r.from, r.to));
    gutter.push(failedLine.range(state.doc.lineAt(r.from).from));
  }
  if (marks.running) {
    const r = clip(marks.running);
    for (let line = state.doc.lineAt(r.from).number; line <= state.doc.lineAt(r.to).number; line++) {
      gutter.push(runningLine.range(state.doc.line(line).from));
    }
  }
  return { text: Decoration.set(text, true), gutter: RangeSet.of(gutter, true) };
}

const marksField = StateField.define<{ text: DecorationSet; gutter: RangeSet<GutterMarker> }>({
  create: () => ({ text: Decoration.none, gutter: RangeSet.empty }),
  update(value, tr) {
    for (const e of tr.effects) if (e.is(setMarks)) return build(tr.state, e.value);
    // Editing clears them: they are about text that is changing
    return tr.docChanged ? { text: Decoration.none, gutter: RangeSet.empty } : value;
  },
  provide: (f) => [EditorView.decorations.from(f, (v) => v.text), gutterLineClass.from(f, (v) => v.gutter)],
});

const editable = new Compartment();

function panelKeys(id: string) {
  return Prec.highest(
    keymap.of([
      { key: "F9", run: () => (keys.get(id)?.run(), true) },
      { key: "Mod-s", run: () => (keys.get(id)?.save(), true) },
    ]),
  );
}

const tabs: [ExecutiveTab, string][] = [
  ["script", "Script"],
  ["statements", "Statements"],
  ["options", "Options"],
];

export function ScriptExecutive({ params }: IDockviewPanelProps<{ id: string }>) {
  const id = params.id;
  const panel = useExecutives((s) => s.panels[id]);
  const host = useRef<HTMLDivElement>(null);
  const view = useRef<EditorView | null>(null);
  const [split, setSplit] = useState(0.7);
  const body = useRef<HTMLDivElement>(null);

  const run = useCallback(() => void runExecutive(id), [id]);

  useEffect(() => {
    if (!host.current) return;
    keys.set(id, { run, save: () => void saveExecutive(id) });
    const state =
      saved.get(id) ??
      EditorState.create({
        extensions: [
          panelKeys(id),
          marksField,
          editable.of([]),
          EditorView.updateListener.of((u) => {
            if (u.docChanged) executiveEdited(id);
          }),
          ...sqlExtensions(() => useExecutives.getState().panels[id]?.registration),
        ],
      });
    const v = new EditorView({ state, parent: host.current });
    view.current = v;
    texts.set(id, {
      read: () => (view.current?.state ?? saved.get(id))?.doc.toString() ?? "",
      write: (text) => {
        const current = view.current;
        if (current) current.dispatch({ changes: { from: 0, to: current.state.doc.length, insert: text }, selection: { anchor: 0 } });
      },
    });
    v.focus();
    return () => {
      saved.set(id, v.state);
      v.destroy();
      view.current = null;
    };
  }, [id, run]);

  // Read-only while it runs: the statements are numbered on the text sent
  const running = !!panel?.run;
  useEffect(() => {
    view.current?.dispatch({ effects: editable.reconfigure(running ? EditorState.readOnly.of(true) : []) });
  }, [running]);

  // The statement running and those that failed, in the text
  const steps = panel?.steps;
  const blocks = panel?.blocks;
  const at = panel?.run ? panel.run.chosen[panel.run.at] : undefined;
  useEffect(() => {
    const v = view.current;
    if (!v || !blocks || !steps) return;
    const byIndex = new Map(blocks.map((b) => [b.index, b]));
    const failed = Object.values(steps)
      .filter((s) => s.status === "failed")
      .map((s) => byIndex.get(s.index))
      .filter((b): b is ScriptBlock => !!b);
    const runningBlock = at === undefined ? null : (byIndex.get(at) ?? null);
    v.dispatch({ effects: setMarks.of({ running: runningBlock, failed }) });
  }, [blocks, steps, at]);

  const select = useCallback(
    (index: number) => {
      const block = useExecutives.getState().panels[id]?.blocks.find((b) => b.index === index);
      const v = view.current;
      if (!block || !v) return;
      patchExecutive(id, { tab: "script" });
      const len = v.state.doc.length;
      const r = { from: Math.min(block.from, len), to: Math.min(block.to, len) };
      v.dispatch({ selection: { anchor: r.from, head: r.to }, effects: EditorView.scrollIntoView(r.from, { y: "center" }) });
      v.focus();
    },
    [id],
  );

  if (!panel) return <div className="grid h-full place-items-center bg-bg text-muted">{t("This Script Executive is closed.")}</div>;

  const startSplit = (e: React.MouseEvent) => {
    e.preventDefault();
    const rect = body.current?.getBoundingClientRect();
    if (!rect) return;
    const onMove = (m: MouseEvent) => setSplit(Math.max(0.2, Math.min(0.9, (m.clientY - rect.top) / rect.height)));
    const onUp = () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
    };
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
  };

  return (
    <div className="flex h-full flex-col bg-panel">
      <Toolbar panel={panel} run={run} />
      {panel.active !== null && <ActiveBar id={id} />}
      <div className="flex h-7 shrink-0 items-end gap-0.5 border-b border-line bg-panel-2 px-2">
        {tabs.map(([tab, label]) => (
          <button
            key={tab}
            className={`h-6 rounded-t px-3 ${panel.tab === tab ? "border border-b-0 border-line bg-panel text-fg" : "text-muted hover:text-fg"}`}
            onClick={() => patchExecutive(id, { tab })}
          >
            {t(label)}
          </button>
        ))}
      </div>
      <div ref={body} className="flex min-h-0 flex-1 flex-col">
        <div className="relative min-h-0" style={{ height: `${split * 100}%` }}>
          {/* The editor stays mounted on the other tabs: its text is the script */}
          <div ref={host} className={`h-full overflow-hidden ${panel.tab === "script" ? "" : "invisible"}`} />
          {panel.tab === "options" && (
            <div className="absolute inset-0 overflow-auto bg-panel">
              <OptionsTab panel={panel} />
            </div>
          )}
        </div>
        <div
          className="h-[5px] shrink-0 cursor-row-resize border-y border-line bg-panel-2 hover:bg-accent/40"
          onMouseDown={startSplit}
          role="separator"
          aria-orientation="horizontal"
        />
        <Messages panel={panel} select={select} />
      </div>
    </div>
  );
}

function Toolbar({ panel, run }: { panel: Executive; run(): void }) {
  const running = !!panel.run;
  const r = panel.run;
  return (
    <div className="flex h-8 shrink-0 items-center gap-0.5 border-b border-line px-1.5">
      <TextButton icon="folder" label={t("Open…")} disabled={running} onClick={() => openExecutiveFile(panel.id)} />
      <TextButton icon="save" label={t("Save")} onClick={() => saveExecutive(panel.id)} />
      <span className="mx-1 h-4 w-px bg-line" />
      <TextButton icon="run" accent label={t("Run (F9)")} disabled={running || panel.active !== null} onClick={run} />
      <TextButton icon="stop" label={t("Stop")} disabled={!running} onClick={() => stopExecutive(panel.id)} />
      <span className="flex-1" />
      {r && (
        <span className="px-2 text-muted tabular-nums">
          {r.at.toLocaleString()} / {r.chosen.length.toLocaleString()}
          {r.failed > 0 && <span className="text-danger"> · {t("{n} failed", { n: r.failed.toLocaleString() })}</span>}
        </span>
      )}
    </div>
  );
}

function ActiveBar({ id }: { id: string }) {
  const [busy, setBusy] = useState(false);
  const end = async (how: "commit" | "rollback") => {
    setBusy(true);
    await endExecutive(id, how);
    setBusy(false);
  };
  return (
    <div className="flex h-8 shrink-0 items-center gap-2 border-b border-line bg-warn/10 px-3">
      <span className="flex-1">{t("The script left a transaction active.")}</span>
      <TextButton icon="rollback" label={t("Roll Back")} disabled={busy} onClick={() => end("rollback")} />
      <TextButton icon="commit" accent label={t("Commit")} disabled={busy} onClick={() => end("commit")} />
    </div>
  );
}

function OptionsTab({ panel }: { panel: Executive }) {
  const set = (o: Partial<ExecutiveOptions>) => patchExecutive(panel.id, (p) => ({ options: { ...p.options, ...o } }));
  const o = panel.options;
  const check = (label: string, value: boolean, on: (v: boolean) => void, note?: string) => (
    <label className="flex items-start gap-2">
      <input type="checkbox" className="mt-0.5 accent-[var(--accent)]" checked={value} onChange={(e) => on(e.target.checked)} />
      <span>
        {label}
        {note && <span className="block text-muted">{note}</span>}
      </span>
    </label>
  );
  return (
    <div className="flex max-w-xl flex-col gap-3 p-4">
      {check(t("Stop at the first error"), o.stopOnError, (v) => set({ stopOnError: v }))}
      {check(
        t("Commit DDL as it runs"),
        o.autoDdl,
        (v) => set({ autoDdl: v }),
        t("As isql's SET AUTODDL ON: what follows a CREATE TABLE can use the table. The script can still turn it off."),
      )}
      {check(t("When an error stops the script, roll back what was not committed"), o.rollbackOnStop, (v) => set({ rollbackOnStop: v }))}
      <fieldset className="flex flex-col gap-1.5">
        <legend className="mb-1">{t("With a transaction still active at the end:")}</legend>
        {(
          [
            ["ask", t("Ask")],
            ["commit", t("Commit")],
            ["rollback", t("Roll back")],
          ] as const
        ).map(([value, label]) => (
          <label key={value} className="flex items-center gap-2">
            <input type="radio" className="accent-[var(--accent)]" checked={o.atEnd === value} onChange={() => set({ atEnd: value })} />
            {label}
          </label>
        ))}
        <span className="text-muted">
          {t("A CONNECT in the script commits what is active before it switches, unless this says to roll back.")}
        </span>
      </fieldset>
    </div>
  );
}

function Messages({ panel, select }: { panel: Executive; select(index: number): void }) {
  const end = useRef<HTMLDivElement>(null);
  const lineOf = new Map(panel.blocks.map((b) => [b.index, b.line]));
  useEffect(() => {
    end.current?.scrollIntoView({ block: "end" });
  }, [panel.messages.length]);
  return (
    <div className="min-h-0 flex-1 overflow-auto bg-panel text-[12px]">
      <table className="w-full border-collapse">
        <thead className="sticky top-0 bg-panel-2 text-left text-muted">
          <tr>
            <th className="w-16 border-b border-line px-2 py-0.5 font-normal">{t("Line")}</th>
            <th className="border-b border-line px-2 py-0.5 font-normal">{t("Message")}</th>
          </tr>
        </thead>
        <tbody>
          {panel.messages.map((m, i) => {
            const line = m.index === undefined ? undefined : lineOf.get(m.index);
            return (
              <tr
                key={i}
                className={`align-top ${m.index !== undefined ? "cursor-pointer hover:bg-hover" : ""}`}
                onClick={() => m.index !== undefined && select(m.index)}
              >
                <td className="px-2 py-0.5 text-muted tabular-nums">{line ?? ""}</td>
                <td className={`px-2 py-0.5 whitespace-pre-wrap ${m.level === "error" ? "text-danger" : ""}`}>
                  {m.level === "error" && <Icon name="exception" size={12} className="mr-1 inline align-[-2px]" />}
                  {m.text}
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
      <div ref={end} />
    </div>
  );
}
