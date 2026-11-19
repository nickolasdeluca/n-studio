// The Script Executive: a script as work in itself, run on a connection of
// its own, all of it or only the statements checked. The script on one
// tab, its statements on another, the options on a third; what failed
// below.
import { Compartment, EditorState, Prec, RangeSet, StateEffect, StateField } from "@codemirror/state";
import { Decoration, EditorView, GutterMarker, gutterLineClass, keymap, type DecorationSet } from "@codemirror/view";
import type { IDockviewPanelProps } from "dockview-react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { Compared, ExecutiveOptions, ExecutiveStep, Operation, ScriptBlock, StepRows, SubjectKind } from "../api";
import { ms } from "../editors";
import {
  checkedOf,
  checkOnly,
  compareExecutive,
  inputName,
  toggleInput,
  toggleInputStatement,
  type InputList,
  chooseCharset,
  chooseDatabase,
  connectsItself,
  endExecutive,
  executiveEdited,
  openExecutiveFile,
  operationLabel,
  outlineExecutive,
  patchExecutive,
  runExecutive,
  runFromFile,
  runningIndex,
  closeDisk,
  saveExecutive,
  stopExecutive,
  takeStartingText,
  texts,
  toggleChecked,
  useExecutives,
  type Executive,
  type ExecutiveTab,
} from "../executive";
import { t } from "../i18n";
import { Icon, kindColor } from "../icons";
import { scriptTree, type TreeItem } from "../scriptTree";
import { lineDiff } from "../lineDiff";
import { prepareCompletion } from "../complete";
import { sqlExtensions } from "../sqlView";
import { useStore } from "../store";
import { useContextMenu } from "./ContextMenu";
import { TextButton } from "./ObjectViewer";

/** Editor states by panel, so a panel that remounts keeps its text and undo */
const saved = new Map<string, EditorState>();
/** What the keys call, by panel: the mounted panel's callbacks */
const keys = new Map<string, { run(): void; save(): void }>();
/** Outlining again after typing, by panel */
const outlines = new Map<string, ReturnType<typeof setTimeout>>();
const OUTLINE_AFTER_MS = 300;

function outlineSoon(id: string) {
  clearTimeout(outlines.get(id));
  outlines.set(id, setTimeout(() => void outlineExecutive(id), OUTLINE_AFTER_MS));
}

export function forgetExecutive(id: string) {
  clearTimeout(outlines.get(id));
  outlines.delete(id);
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
      { key: "Mod-s", run: () => (keys.get(id)?.save(), true) },
    ]),
  );
}

const tabs: [ExecutiveTab, string][] = [
  ["script", "Script"],
  ["statements", "Statements"],
  ["compare", "Compare"],
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
        doc: takeStartingText(id),
        extensions: [
          panelKeys(id),
          marksField,
          editable.of([]),
          EditorView.updateListener.of((u) => {
            if (!u.docChanged) return;
            executiveEdited(id);
            outlineSoon(id);
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
      prepend: (text) => {
        const current = view.current;
        if (!current) return;
        current.dispatch({ changes: { from: 0, insert: text }, selection: { anchor: text.length }, scrollIntoView: true });
        current.focus();
      },
    });
    v.focus();
    if (v.state.doc.length > 0) outlineSoon(id);
    return () => {
      saved.set(id, v.state);
      v.destroy();
      view.current = null;
    };
  }, [id, run]);

  // Completion from the catalog of the database chosen, once connected
  const registration = panel?.registration;
  const connected = useStore((s) => !!registration && !!s.servers[registration]);
  useEffect(() => {
    if (registration && connected) prepareCompletion(registration);
  }, [registration, connected]);

  // Read-only while it runs: the statements are numbered on the text sent
  const running = !!panel?.run;
  useEffect(() => {
    view.current?.dispatch({ effects: editable.reconfigure(running ? EditorState.readOnly.of(true) : []) });
  }, [running]);

  // The statement running and those that failed, in the text
  const steps = panel?.steps;
  const blocks = panel?.blocks;
  const at = runningIndex(panel?.run ?? null);
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
      const panel = useExecutives.getState().panels[id];
      // A file run from disk has no text here: its statement is in the list
      if (panel?.disk) {
        patchExecutive(id, { tab: "statements", focus: index });
        return;
      }
      const block = panel?.blocks.find((b) => b.index === index);
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
    <div
      className="flex h-full flex-col bg-panel"
      onKeyDownCapture={(e) => {
        // IBExpert's keys, wherever in the panel the keyboard is
        if (e.key !== "F9" || e.ctrlKey || e.metaKey || e.altKey) return;
        e.preventDefault();
        e.stopPropagation();
        if (e.shiftKey) void runFromFile(id);
        else run();
      }}
    >
      <Toolbar panel={panel} run={run} />
      {panel.active !== null && <ActiveBar id={id} />}
      <div className="flex min-h-0 flex-1">
        {panel.explorer && (
          <div className="w-60 shrink-0 overflow-auto border-r border-line bg-panel-2/40">
            <Explorer panel={panel} select={select} />
          </div>
        )}
        <div className="flex min-w-0 flex-1 flex-col">
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
              <div ref={host} className={`h-full overflow-hidden ${panel.tab === "script" && !panel.disk ? "" : "invisible"}`} />
              {panel.tab === "script" && panel.disk && (
                <div className="absolute inset-0 bg-panel">
                  <DiskNotice panel={panel} />
                </div>
              )}
              {panel.tab === "statements" && (
                <div className="absolute inset-0 bg-panel">
                  <StatementsTab panel={panel} select={select} />
                </div>
              )}
              {panel.tab === "compare" && (
                <div className="absolute inset-0 bg-panel">
                  <CompareTab panel={panel} select={select} />
                </div>
              )}
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
      </div>
    </div>
  );
}

const selectClass = "h-6 max-w-[14rem] rounded border border-line bg-panel px-1 text-fg disabled:text-faint";

function Toolbar({ panel, run }: { panel: Executive; run(): void }) {
  const running = !!panel.run;
  const r = panel.run;
  const registrations = useStore((s) => s.registrations);
  const charsets = useStore((s) => s.charsets);
  const locked = running || panel.active !== null;
  return (
    <div className="flex h-8 shrink-0 items-center gap-0.5 overflow-x-auto border-b border-line px-1.5 [&>button]:shrink-0 [&>button]:whitespace-nowrap">
      <select
        className={selectClass}
        aria-label={t("Database")}
        title={panel.active !== null ? t("Commit or roll back first.") : t("The database the script runs on")}
        disabled={locked}
        value={panel.registration ?? ""}
        onChange={(e) => chooseDatabase(panel.id, e.target.value || null)}
      >
        <option value="">{connectsItself(panel) ? t("(from the script)") : t("(no database)")}</option>
        {registrations.map((reg) => (
          <option key={reg.id} value={reg.id}>
            {reg.name}
          </option>
        ))}
      </select>
      <TextButton
        icon="plug"
        label={t("Add CONNECT…")}
        disabled={running || !!panel.disk}
        onClick={() => useStore.getState().openDialog({ type: "connectStatement", panel: panel.id })}
      />
      <span className="mx-1 h-4 w-px bg-line" />
      <TextButton icon="folder" label={t("Open…")} disabled={running} onClick={() => openExecutiveFile(panel.id)} />
      <TextButton icon="save" label={t("Save")} onClick={() => saveExecutive(panel.id)} />
      <span className="mx-1 h-4 w-px bg-line" />
      <TextButton icon="run" accent label={t("Run")} disabled={running || panel.active !== null} onClick={run} />
      <TextButton
        icon="run"
        label={t("Run Checked")}
        disabled={running || panel.active !== null || panel.blocks.length === 0}
        onClick={() => runExecutive(panel.id, Object.keys(panel.unchecked).length === 0 ? null : checkedOf(panel))}
      />
      <TextButton
        icon="open"
        label={t("Run from File…")}
        disabled={running || panel.active !== null}
        onClick={() => runFromFile(panel.id)}
      />
      <TextButton icon="stop" label={t("Stop")} disabled={!running} onClick={() => stopExecutive(panel.id)} />
      <span className="mx-1 h-4 w-px bg-line" />
      <TextButton
        icon="panel"
        accent={panel.explorer}
        label={t("Explorer")}
        onClick={() => patchExecutive(panel.id, (p) => ({ explorer: !p.explorer }))}
      />
      <span className="flex-1" />
      <label className="flex shrink-0 items-center gap-1 px-1 text-muted">
        {t("Charset")}
        <select
          className={selectClass}
          disabled={locked}
          title={t("The character set the script is sent in, and a file from disk read in")}
          value={panel.charset ?? ""}
          onChange={(e) => void chooseCharset(panel.id, e.target.value || null)}
        >
          <option value="">{t("(the database's)")}</option>
          {charsets.map((c) => (
            <option key={c} value={c}>
              {c}
            </option>
          ))}
        </select>
      </label>
      {r && (
        <span className="px-2 text-muted tabular-nums">
          {r.at.toLocaleString()} / {r.total.toLocaleString()}
          {r.failed > 0 && <span className="text-danger"> · {t("{n} failed", { n: r.failed.toLocaleString() })}</span>}
        </span>
      )}
    </div>
  );
}

function size(bytes: number) {
  if (bytes >= 1 << 20) return `${(bytes / (1 << 20)).toFixed(1)} MB`;
  return `${Math.max(1, Math.round(bytes / 1024))} KB`;
}

/** In place of the editor while a file is run from disk */
function DiskNotice({ panel }: { panel: Executive }) {
  const disk = panel.disk!;
  return (
    <div className="flex h-full flex-col items-center justify-center gap-2 p-6 text-center">
      <Icon name="open" size={22} className="text-muted" />
      <p>{t("This script is run from disk, without loading it into the editor.")}</p>
      <p className="font-mono text-[12px] break-all text-muted">{disk.path}</p>
      <p className="text-muted">
        {t("{size} · {n} statement(s) · read as {charset}", {
          size: size(disk.size),
          n: disk.total.toLocaleString(),
          charset: disk.readAs,
        })}
      </p>
      <div className="mt-2 flex gap-1">
        <TextButton icon="run" accent label={t("Run Again")} disabled={!!panel.run || panel.active !== null} onClick={() => runExecutive(panel.id)} />
        <TextButton icon="sql" label={t("Back to the Editor")} disabled={!!panel.run} onClick={() => closeDisk(panel.id)} />
      </div>
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
      {check(
        t("Run SHELL commands"),
        !!o.allowShell,
        (v) => set({ allowShell: v }),
        t("A script's SHELL starts a program, in the script's folder. Off, it is passed over and said so: turn it on only for scripts you trust."),
      )}
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
            const line = m.line;
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

// ── Statements ────────────────────────────────────────────────────────────

const ROW_H = 24;

type RowState = "waiting" | "running" | ExecutiveStep["status"] | null;

function stateOf(panel: Executive, index: number): RowState {
  const step = panel.steps[index];
  if (step) return step.status;
  const run = panel.run;
  if (!run) return null;
  if (runningIndex(run) === index) return "running";
  return (run.picked ? run.picked.has(index) : index >= run.at) ? "waiting" : null;
}

const stateLabels: Record<Exclude<RowState, null>, string> = {
  waiting: "Waiting",
  running: "Running",
  done: "Done",
  applied: "Applied",
  skipped: "Skipped",
  failed: "Failed",
};

const stateClass: Record<Exclude<RowState, null>, string> = {
  waiting: "text-faint",
  running: "text-accent",
  done: "text-ok",
  applied: "text-ok",
  skipped: "text-muted",
  failed: "text-danger",
};

function rowsAffected(step: ExecutiveStep | undefined): string {
  if (step?.status !== "done") return "";
  const r = step.records;
  const changed = r.inserted + r.updated + r.deleted;
  return changed > 0 ? changed.toLocaleString() : "";
}

function StatementsTab({ panel, select }: { panel: Executive; select(index: number): void }) {
  const scroll = useRef<HTMLDivElement>(null);
  const showMenu = useContextMenu((m) => m.show);
  const blocks = panel.blocks;
  const rows = useVirtualizer({
    count: blocks.length,
    getScrollElement: () => scroll.current,
    estimateSize: () => ROW_H,
    overscan: 20,
  });
  const checked = blocks.length - Object.keys(panel.unchecked).length;
  const anyFailed = Object.values(panel.steps).some((s) => s.status === "failed");
  const running = !!panel.run;

  // The statement running stays in sight
  const at = runningIndex(panel.run);
  useEffect(() => {
    if (at === undefined) return;
    const row = blocks.findIndex((b) => b.index === at);
    if (row >= 0) rows.scrollToIndex(row, { align: "auto" });
  }, [at, blocks, rows]);

  // A statement asked for from a message or the explorer
  const focus = panel.focus;
  useEffect(() => {
    if (focus === null) return;
    const row = blocks.findIndex((b) => b.index === focus);
    if (row >= 0) rows.scrollToIndex(row, { align: "center" });
    patchExecutive(panel.id, { focus: null });
  }, [focus, blocks, rows, panel.id]);

  const menu = (e: React.MouseEvent, block: ScriptBlock) => {
    e.preventDefault();
    showMenu(e.clientX, e.clientY, [
      { label: t("Go to Statement"), icon: "sql", onSelect: () => select(block.index) },
      "separator",
      { label: t("Check from Here"), disabled: running, onSelect: () => checkOnly(panel.id, (i) => i >= block.index) },
      { label: t("Check Only This"), disabled: running, onSelect: () => checkOnly(panel.id, (i) => i === block.index) },
    ]);
  };

  const cols = "grid grid-cols-[28px_56px_minmax(10rem,18rem)_minmax(0,1fr)_72px_72px_64px] items-center";
  return (
    <div className="flex h-full flex-col text-[12px]">
      <div className="flex h-8 shrink-0 items-center gap-0.5 border-b border-line px-1.5">
        <TextButton icon="commit" label={t("Check All")} disabled={running} onClick={() => checkOnly(panel.id, () => true)} />
        <TextButton icon="stop" label={t("Check None")} disabled={running} onClick={() => checkOnly(panel.id, () => false)} />
        <TextButton
          icon="exception"
          label={t("Check Only Failed")}
          disabled={running || !anyFailed}
          onClick={() => checkOnly(panel.id, (i) => panel.steps[i]?.status === "failed")}
        />
        <span className="flex-1" />
        <span className="px-2 text-muted tabular-nums">
          {t("{checked} of {total} checked", { checked: checked.toLocaleString(), total: blocks.length.toLocaleString() })}
        </span>
      </div>
      <div className={`${cols} h-6 shrink-0 border-b border-line bg-panel-2 text-muted`}>
        <span className="grid place-items-center">
          <input
            type="checkbox"
            aria-label={t("Check All")}
            className="accent-[var(--accent)]"
            disabled={running || blocks.length === 0}
            checked={blocks.length > 0 && checked === blocks.length}
            ref={(el) => {
              if (el) el.indeterminate = checked > 0 && checked < blocks.length;
            }}
            onChange={(e) => checkOnly(panel.id, () => e.target.checked)}
          />
        </span>
        <span className="px-1.5 text-right">{t("Line")}</span>
        <span className="px-1.5">{t("Operation")}</span>
        <span className="px-1.5">{t("Statement")}</span>
        <span className="px-1.5">{t("State")}</span>
        <span className="px-1.5 text-right">{t("Time")}</span>
        <span className="px-1.5 text-right">{t("Rows")}</span>
      </div>
      {blocks.length === 0 ? (
        <div className="grid flex-1 place-items-center text-muted">{t("The script has no statements yet.")}</div>
      ) : (
        <div ref={scroll} className="min-h-0 flex-1 overflow-auto">
          <div className="relative w-full" style={{ height: rows.getTotalSize() }}>
            {rows.getVirtualItems().map((item) => {
              const block = blocks[item.index];
              const step = panel.steps[block.index];
              const state = stateOf(panel, block.index);
              return (
                <div
                  key={block.index}
                  data-index={item.index}
                  ref={rows.measureElement}
                  className="absolute left-0 w-full border-b border-line/50"
                  style={{ transform: `translateY(${item.start}px)` }}
                >
                  <div
                    className={`${cols} h-6 cursor-pointer hover:bg-hover ${state === "running" ? "bg-accent/10" : ""}`}
                    onClick={() => select(block.index)}
                    onContextMenu={(e) => menu(e, block)}
                  >
                    <span className="grid place-items-center" onClick={(e) => e.stopPropagation()}>
                      <input
                        type="checkbox"
                        aria-label={t("Run statement {n}", { n: block.index + 1 })}
                        className="accent-[var(--accent)]"
                        disabled={running}
                        checked={!panel.unchecked[block.index]}
                        onChange={() => toggleChecked(panel.id, block.index)}
                      />
                    </span>
                    <span className="px-1.5 text-right text-muted tabular-nums">{block.line}</span>
                    <span className="flex min-w-0 items-center gap-1 px-1.5">
                      {inputName(block) !== null && (
                        <button
                          type="button"
                          aria-label={panel.inputs?.[block.index]?.open ? t("Hide its file's statements") : t("Show its file's statements")}
                          aria-expanded={!!panel.inputs?.[block.index]?.open}
                          className="grid size-4 shrink-0 place-items-center rounded text-muted hover:bg-hover hover:text-fg"
                          onClick={(e) => {
                            e.stopPropagation();
                            void toggleInput(panel.id, block.index, inputName(block)!);
                          }}
                        >
                          <Icon name="chevron" size={11} className={panel.inputs?.[block.index]?.open ? "rotate-90" : ""} />
                        </button>
                      )}
                      <span className="truncate">{operationLabel(block) || <span className="text-faint">{block.kind === "isql" ? t("isql command") : ""}</span>}</span>
                    </span>
                    <span className="truncate px-1.5 font-mono text-[11.5px] text-muted">{block.summary}</span>
                    <span className={`px-1.5 ${state ? stateClass[state] : ""}`}>{state ? t(stateLabels[state]) : ""}</span>
                    <span className="px-1.5 text-right text-muted tabular-nums">{step ? ms(step.ms) : ""}</span>
                    <span className="px-1.5 text-right tabular-nums">{rowsAffected(step)}</span>
                  </div>
                  {step?.status === "failed" && (
                    <div className="py-1 pr-2 pl-[84px] whitespace-pre-wrap text-danger">{step.message}</div>
                  )}
                  {(step?.status === "skipped" || step?.status === "applied") && step.note && <div className="py-1 pr-2 pl-[84px] text-muted">{step.note}</div>}
                  {step?.status === "done" && step.rows && <StepTable rows={step.rows} />}
                  {panel.inputs?.[block.index]?.open && (
                    <InputRows
                      list={panel.inputs[block.index]}
                      cols={cols}
                      disabled={running || !!panel.unchecked[block.index]}
                      toggle={(inner) => toggleInputStatement(panel.id, block.index, inner)}
                    />
                  )}
                </div>
              );
            })}
          </div>
        </div>
      )}
    </div>
  );
}

// ── Explorer ──────────────────────────────────────────────────────────────

const groupLabels: Record<SubjectKind, string> = {
  table: "Tables",
  view: "Views",
  procedure: "Procedures",
  function: "Functions",
  package: "Packages",
  trigger: "Triggers",
  domain: "Domains",
  generator: "Generators",
  exception: "Exceptions",
  index: "Indices",
  role: "Roles",
  user: "Users",
  udf: "UDFs",
  block: "Firebird blocks",
  database: "Database",
};

const groupIcons: Partial<Record<SubjectKind, string>> = { user: "role", block: "sql" };

const shortVerbs: Record<Operation, string> = {
  create: "Create",
  alter: "Alter",
  createOrAlter: "Create or alter",
  recreate: "Recreate",
  drop: "Drop",
  comment: "Comment",
  grant: "Grant",
  revoke: "Revoke",
  insert: "Insert",
  update: "Update",
  updateOrInsert: "Update or insert",
  delete: "Delete",
  merge: "Merge",
  select: "Select",
  execute: "Execute",
  commit: "Commit",
  rollback: "Roll back",
  setGenerator: "Set",
  connect: "Connect",
  other: "Other",
};

function itemLabel(item: TreeItem) {
  const verb = t(shortVerbs[item.operation]);
  if (item.index_on) return `${verb} ${t("index")} ${item.detail}`;
  return item.detail ? `${verb} ${item.detail}` : verb;
}

function Explorer({ panel, select }: { panel: Executive; select(index: number): void }) {
  const tree = useMemo(() => scriptTree(panel.blocks), [panel.blocks]);
  const lineOf = useMemo(() => new Map(panel.blocks.map((b) => [b.index, b.line])), [panel.blocks]);
  const [open, setOpen] = useState<Record<string, boolean>>({});
  const toggle = (key: string) => setOpen((o) => ({ ...o, [key]: !o[key] }));
  const failed = (index: number) => panel.steps[index]?.status === "failed";

  const row = "flex h-[22px] w-full items-center gap-1 pr-2 text-left hover:bg-hover";
  const chevron = (isOpen: boolean) => (
    <Icon name="chevron" size={11} className={`shrink-0 text-muted transition-transform ${isOpen ? "rotate-90" : ""}`} />
  );
  const leaf = (item: TreeItem, depth: number, extra = "") => (
    <button
      key={`${item.index}-${item.detail}`}
      className={`${row} ${failed(item.index) ? "text-danger" : ""}`}
      style={{ paddingLeft: depth * 14 + 8 }}
      title={t("Line {line}", { line: lineOf.get(item.index) ?? "" })}
      onClick={() => select(item.index)}
    >
      <span className="truncate">{itemLabel(item)}{extra}</span>
    </button>
  );

  return (
    <div className="py-1 text-[12px] select-none">
      {tree.map((group) => {
        const count = group.objects.length + group.loose.length;
        const isOpen = !!open[group.kind] && count > 0;
        return (
          <div key={group.kind}>
            <button className={`${row} pl-1 ${count === 0 ? "text-faint" : ""}`} onClick={() => toggle(group.kind)} disabled={count === 0}>
              {chevron(isOpen)}
              <Icon name={groupIcons[group.kind] ?? group.kind} size={13} className={`shrink-0 ${count > 0 ? (kindColor[group.kind] ?? "") : ""}`} />
              <span className="truncate">{t(groupLabels[group.kind])}</span>
              {count > 0 && <span className="text-muted">({count.toLocaleString()})</span>}
            </button>
            {isOpen &&
              group.objects.map((object) => {
                const key = `${group.kind}:${object.name}`;
                const objectOpen = !!open[key];
                return (
                  <div key={key}>
                    <button className={row} style={{ paddingLeft: 18 }} onClick={() => toggle(key)}>
                      {chevron(objectOpen)}
                      <span className={`truncate ${object.items.some((i) => failed(i.index)) ? "text-danger" : ""}`}>{object.name}</span>
                      <span className="text-muted">({object.items.length.toLocaleString()})</span>
                    </button>
                    {objectOpen && object.items.map((item) => leaf(item, 3))}
                  </div>
                );
              })}
            {isOpen && group.loose.map((item) => leaf(item, 2, ` · ${t("line {line}", { line: lineOf.get(item.index) ?? "" })}`))}
          </div>
        );
      })}
    </div>
  );
}

/** The rows a select returned, under its row: the first ones only */
function StepTable({ rows }: { rows: StepRows }) {
  return (
    <div className="pr-2 pb-1.5 pl-[84px]">
      <div className="scroll-thin max-h-56 overflow-auto rounded border border-line">
        <table className="border-collapse font-mono text-[11.5px] select-text">
          <thead className="sticky top-0 bg-panel-2 text-left text-muted">
            <tr>
              {rows.columns.map((c, i) => (
                <th key={i} className="px-2 py-0.5 font-normal whitespace-nowrap">
                  {c}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {rows.rows.map((row, r) => (
              <tr key={r} className="border-t border-line">
                {row.map((v, c) => (
                  <td key={c} className={`max-w-[40ch] truncate px-2 py-0.5 whitespace-nowrap ${v === "<null>" ? "text-faint" : ""}`} title={v}>
                    {v}
                  </td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      {rows.more && <p className="pt-0.5 text-muted">{t("The first {n} rows: it returned more.", { n: rows.rows.length })}</p>}
    </div>
  );
}

/** The statements of the file an INPUT names, to leave some out */
function InputRows({ list, cols, disabled, toggle }: { list: InputList; cols: string; disabled: boolean; toggle(inner: number): void }) {
  if (list.error) return <div className="py-1 pr-2 pl-[84px] text-danger">{list.error}</div>;
  if (!list.outline) return <div className="py-1 pr-2 pl-[84px] text-muted">{t("Reading…")}</div>;
  const { outline } = list;
  return (
    <div className="border-t border-line/50 bg-panel-2/60">
      <div className="truncate py-0.5 pr-2 pl-[84px] text-muted" title={outline.file}>
        {outline.file}
      </div>
      {outline.blocks.map((b) => (
        <div key={b.index} className={`${cols} h-6`}>
          <span />
          <span className="grid place-items-center">
            <input
              type="checkbox"
              aria-label={t("Run statement {n} of the file", { n: b.index + 1 })}
              className="accent-[var(--accent)]"
              disabled={disabled}
              checked={!list.unchecked[b.index]}
              onChange={() => toggle(b.index)}
            />
          </span>
          <span className="truncate px-1.5 text-muted">
            {t("line {line}", { line: b.line })} · {operationLabel(b) || (b.kind === "isql" ? t("isql command") : "")}
          </span>
          <span className="col-span-4 truncate px-1.5 font-mono text-[11.5px] text-muted">{b.summary}</span>
        </div>
      ))}
      {outline.total > outline.blocks.length && (
        <div className="py-0.5 pr-2 pl-[84px] text-muted">
          {t("Only its first {n} statements are listed; those past them always run.", { n: outline.blocks.length })}
        </div>
      )}
    </div>
  );
}

const compareStates = { new: "New", same: "Same", differs: "Differs" } as const;
const stateOfCompared = (c: Compared) => (c.current === null ? "new" : c.same ? "same" : "differs");

/** What the script creates or changes, beside what the database has */
function CompareTab({ panel, select }: { panel: Executive; select(index: number): void }) {
  const compared = panel.compared;
  const [open, setOpen] = useState<number | null>(null);
  const items = compared?.items ?? [];
  const shown = items.find((c) => c.index === open) ?? null;
  return (
    <div className="flex h-full flex-col">
      <div className="flex h-8 shrink-0 items-center gap-2 border-b border-line px-2">
        <TextButton
          icon="refresh"
          label={compared ? t("Compare again") : t("Compare with the database")}
          disabled={!!compared?.busy || !!panel.run}
          onClick={() => compareExecutive(panel.id)}
        />
        {compared?.busy && <span className="text-muted">{t("Comparing…")}</span>}
        {compared?.items && !compared.busy && (
          <span className="text-muted">
            {t("{n} new, {d} differ, {s} the same", {
              n: items.filter((c) => c.current === null).length,
              d: items.filter((c) => c.current !== null && !c.same).length,
              s: items.filter((c) => c.same).length,
            })}
          </span>
        )}
      </div>
      {compared?.error ? (
        <div className="p-3 text-danger">{compared.error}</div>
      ) : !compared?.items ? (
        <div className="grid flex-1 place-items-center px-6 text-center text-muted">
          {t("Compares each object the script creates or changes with what the database has of it now, before you run it.")}
        </div>
      ) : items.length === 0 ? (
        <div className="grid flex-1 place-items-center text-muted">{t("The script creates or changes no object.")}</div>
      ) : (
        <div className="flex min-h-0 flex-1">
          <div className="scroll-thin w-72 shrink-0 overflow-auto border-r border-line">
            {items.map((c) => {
              const state = stateOfCompared(c);
              return (
                <button
                  key={c.index}
                  type="button"
                  className={`flex h-6 w-full items-center gap-1.5 px-2 text-left hover:bg-hover ${open === c.index ? "bg-accent/15" : ""}`}
                  onClick={() => setOpen(c.index)}
                  onDoubleClick={() => select(c.index)}
                  title={t("Double-click to find it in the script")}
                >
                  <Icon name={c.kind} size={13} className={kindColor[c.kind] ?? ""} />
                  <span className="min-w-0 flex-1 truncate">{c.name}</span>
                  <span className={state === "same" ? "text-muted" : state === "new" ? "text-ok" : "text-warn"}>
                    {t(compareStates[state])}
                  </span>
                </button>
              );
            })}
          </div>
          <div className="scroll-thin min-w-0 flex-1 overflow-auto">
            {shown ? <CompareDiff item={shown} /> : <div className="grid h-full place-items-center text-muted">{t("Choose an object to see what the script changes in it.")}</div>}
          </div>
        </div>
      )}
    </div>
  );
}

/** The database's statement against the script's, line by line */
function CompareDiff({ item }: { item: Compared }) {
  const lines = useMemo(() => (item.current === null ? null : lineDiff(item.current, item.script)), [item]);
  const mark = { same: "", removed: "bg-danger/12 text-danger", added: "bg-ok/12 text-ok" } as const;
  const sign = { same: " ", removed: "-", added: "+" } as const;
  return (
    <div className="p-2">
      <p className="pb-1.5 text-muted">
        {item.current === null
          ? t("The database has no such object: the script creates it.")
          : t("Lines with - are the database's, lines with + the script's.")}
      </p>
      <pre className="font-mono text-[11.5px] leading-[1.5] select-text">
        {lines === null
          ? item.script
          : lines.map((l, i) => (
              <div key={i} className={mark[l.kind]}>
                {sign[l.kind]} {l.text}
              </div>
            ))}
      </pre>
    </div>
  );
}
