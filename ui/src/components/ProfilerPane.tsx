// The profiler of Firebird 5: a session on the registration's own
// connection measures what its editors run, and what it measured is read
// back as statements, PSQL lines and the steps of each plan.
import { useEffect, useMemo, useState } from "react";
import { create } from "zustand";
import { api, errorText, type Profile, type ProfiledStatement, type ProfileSession } from "../api";
import { t } from "../i18n";
import { TextButton } from "./ObjectViewer";

/** The session open on each registration's connection, and since when */
const useOpen = create<{ open: Record<string, { id: number; since: Date }> }>(() => ({ open: {} }));

const card = "rounded-md border border-line bg-panel";
const mono = "font-mono text-[11.5px]";
const cell = "border-t border-line px-2 py-1";

/** Nanoseconds, as the profiler counts them */
function time(ns: number): string {
  const ms = ns / 1e6;
  if (ms >= 60_000) return `${(ms / 60_000).toFixed(1)} min`;
  if (ms >= 1000) return `${(ms / 1000).toFixed(2)} s`;
  if (ms >= 1) return `${ms.toFixed(ms >= 100 ? 0 : 1)} ms`;
  return `${Math.round(ns / 1000)} µs`;
}

const n = (value: number) => value.toLocaleString();

/** A statement by what it is: its routine, or its text's first words */
function label(s: ProfiledStatement): string {
  if (s.routine) return [s.package, s.routine].filter(Boolean).join(".");
  return (s.sql ?? "").split(/\s+/).join(" ").trim().slice(0, 160) || `#${s.id}`;
}

export function ProfilerPane({ registration }: { registration: string }) {
  const open = useOpen((s) => s.open[registration]);
  const [sessions, setSessions] = useState<ProfileSession[]>([]);
  const [picked, setPicked] = useState<number | null>(null);
  const [profile, setProfile] = useState<Profile | null>(null);
  const [statement, setStatement] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [deleting, setDeleting] = useState(false);

  const list = async (pick?: number) => {
    const found = await api.profilerSessions(registration);
    setSessions(found);
    setPicked((was) => pick ?? was ?? found[0]?.id ?? null);
  };

  const act = async (what: () => Promise<unknown>) => {
    setBusy(true);
    setError(null);
    try {
      await what();
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  };

  useEffect(() => {
    void act(() => list());
  }, [registration]);

  useEffect(() => {
    setProfile(null);
    setStatement(null);
    setDeleting(false);
    if (picked === null) return;
    let live = true;
    api.profilerReport(registration, picked).then(
      (p) => {
        if (!live) return;
        setProfile(p);
        setStatement(p.statements[0]?.id ?? null);
      },
      (e) => live && setError(errorText(e)),
    );
    return () => {
      live = false;
    };
  }, [registration, picked]);

  const start = () =>
    act(async () => {
      const id = await api.profilerStart(registration);
      useOpen.setState((s) => ({ open: { ...s.open, [registration]: { id, since: new Date() } } }));
    });

  const finish = (discard: boolean) =>
    act(async () => {
      const id = open?.id;
      try {
        await api.profilerFinish(registration, discard);
      } finally {
        useOpen.setState((s) => {
          const rest = { ...s.open };
          delete rest[registration];
          return { open: rest };
        });
      }
      await list(discard ? undefined : id);
    });

  const remove = () =>
    act(async () => {
      if (picked === null) return;
      await api.profilerDelete(registration, picked);
      setPicked(null);
      await list();
    });

  return (
    <div className="scroll-thin flex h-full flex-col gap-3 overflow-auto bg-bg p-3">
      <div className={`${card} flex flex-wrap items-center gap-3 px-3 py-2`}>
        {open ? (
          <>
            <span className="flex items-center gap-1.5 font-semibold text-danger">
              <span aria-hidden className="size-[9px] rounded-full bg-danger" />
              {t("Profiling since {time}", { time: open.since.toLocaleTimeString() })}
            </span>
            <TextButton icon="stop" label={t("Stop and Report")} disabled={busy} onClick={() => finish(false)} />
            <TextButton icon="rollback" label={t("Cancel")} disabled={busy} onClick={() => finish(true)} />
            <span className="text-muted">{t("Run what is to be measured in this database's editors.")}</span>
          </>
        ) : (
          <>
            <TextButton icon="run" accent label={t("Start Profiling")} disabled={busy} onClick={start} />
            <span className="max-w-[80ch] text-muted">
              {t(
                "Measures what this database's editors run from now on, and the routines and triggers they call: by line of PSQL and by step of each plan. When it stops, what it measured is written to tables of the database (PLG$PROF_*), created the first time.",
              )}
            </span>
          </>
        )}
      </div>
      {error && (
        <p className="whitespace-pre-wrap text-danger select-text" role="alert">
          {error}
        </p>
      )}
      <div className="flex min-h-0 flex-wrap items-start gap-3">
        <div className={`${card} min-w-0 flex-[1_1_240px]`}>
          <div className="border-b border-line px-3 py-2 font-semibold">{t("Sessions")}</div>
          {sessions.length === 0 && <p className="px-3 py-2 text-muted">{t("No session was written down in this database.")}</p>}
          {sessions.map((s) => (
            <button
              key={s.id}
              type="button"
              onClick={() => setPicked(s.id)}
              className={`flex w-full flex-col border-t border-line px-3 py-1.5 text-left first:border-t-0 ${picked === s.id ? "bg-selected" : "hover:bg-hover"}`}
            >
              <span className="tabular-nums">
                #{s.id} · {s.started ?? "—"}
              </span>
              <span className="truncate text-muted">{[s.description, s.user, `ATT_${s.attachment}`].filter(Boolean).join(" · ")}</span>
            </button>
          ))}
        </div>
        <div className="flex min-w-0 flex-[999_1_640px] flex-col gap-3">
          {profile ? (
            <Report profile={profile} statement={statement} pick={setStatement} />
          ) : (
            picked !== null && <p className="text-muted">{t("Reading…")}</p>
          )}
          {profile && (
            <div className="flex items-center gap-2">
              {deleting ? (
                <>
                  <span>{t("Delete what session #{id} wrote down?", { id: profile.session.id })}</span>
                  <TextButton icon="trash" label={t("Delete")} disabled={busy} onClick={remove} />
                  <TextButton icon="rollback" label={t("Keep")} onClick={() => setDeleting(false)} />
                </>
              ) : (
                <TextButton icon="trash" label={t("Delete Session…")} disabled={busy} onClick={() => setDeleting(true)} />
              )}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

function Report({ profile, statement, pick }: { profile: Profile; statement: number | null; pick(id: number): void }) {
  const byId = useMemo(() => new Map(profile.statements.map((s) => [s.id, s])), [profile]);
  // What the top-level statements took is the whole: a routine's time is in its caller's too
  const whole = Math.max(1, profile.statements.filter((s) => s.parent === null).reduce((sum, s) => sum + s.total, 0));
  const lines = profile.lines.filter((l) => statement === null || l.statement === statement);
  const sources = profile.sources.filter((s) => s.statement === statement);
  const chosen = statement === null ? undefined : byId.get(statement);
  // A routine or trigger is shown under what called it
  const depth = (s: ProfiledStatement): number => {
    let d = 0;
    for (let p = s.parent; p !== null && d < 8; p = byId.get(p)?.parent ?? null) d++;
    return d;
  };
  return (
    <>
      <div className={card}>
        <div className="border-b border-line px-3 py-2 font-semibold">{t("Statements")}</div>
        <table className="w-full table-fixed border-collapse">
          <colgroup>
            <col className="w-24" />
            <col />
            <col className="w-20" />
            <col className="w-24" />
            <col className="w-12" />
          </colgroup>
          <thead className="text-left text-muted">
            <tr>
              <th className="px-2 py-1 font-normal">{t("Kind")}</th>
              <th className="px-2 py-1 font-normal">{t("Statement")}</th>
              <th className="px-2 py-1 text-right font-normal">{t("Runs")}</th>
              <th className="px-2 py-1 text-right font-normal">{t("Time")}</th>
              <th className="px-2 py-1 text-right font-normal">%</th>
            </tr>
          </thead>
          <tbody>
            {profile.statements.map((s) => (
              <tr
                key={s.id}
                onClick={() => pick(s.id)}
                className={`cursor-default ${statement === s.id ? "bg-selected" : "hover:bg-hover"}`}
              >
                <td className={`${cell} text-muted`}>{s.kind ?? ""}</td>
                <td className={`${cell} ${mono} truncate`} title={s.sql ?? label(s)} style={{ paddingLeft: 8 + depth(s) * 16 }}>
                  {label(s)}
                </td>
                <td className={`${cell} text-right tabular-nums`}>{n(s.runs)}</td>
                <td className={`${cell} text-right tabular-nums`}>{time(s.total)}</td>
                <td className={`${cell} text-right tabular-nums text-muted`}>{s.parent === null ? `${Math.round((s.total * 100) / whole)}` : ""}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      {chosen && (
        <div className={card}>
          <div className="border-b border-line px-3 py-2 font-semibold">{t("Plan of {statement}", { statement: label(chosen) })}</div>
          {sources.length === 0 ? (
            <p className="px-3 py-2 text-muted">{t("It read no records.")}</p>
          ) : (
            <table className="w-full border-collapse">
              <thead className="text-left text-muted">
                <tr>
                  <th className="px-2 py-1 font-normal">{t("Step")}</th>
                  <th className="px-2 py-1 text-right font-normal">{t("Opens")}</th>
                  <th className="px-2 py-1 text-right font-normal">{t("Fetches")}</th>
                  <th className="px-2 py-1 text-right font-normal">{t("Time")}</th>
                </tr>
              </thead>
              <tbody>
                {sources.map((s, i) => (
                  <tr key={`${s.cursor}-${s.id}`}>
                    <td className={`${cell} ${mono}`} style={{ paddingLeft: 8 + s.level * 16 }}>
                      {(i === 0 || sources[i - 1].cursor !== s.cursor) && s.level === 0 && (
                        <span className="mr-2 text-muted">
                          {s.cursor_name ?? (s.cursor_line ? t("line {line}", { line: s.cursor_line }) : t("cursor {n}", { n: s.cursor }))}
                        </span>
                      )}
                      {s.access_path ?? ""}
                    </td>
                    <td className={`${cell} text-right tabular-nums`}>{n(s.opens)}</td>
                    <td className={`${cell} text-right tabular-nums`}>{n(s.fetches)}</td>
                    <td className={`${cell} text-right tabular-nums`}>{time(s.total)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </div>
      )}
      {lines.length > 0 && (
        <div className={card}>
          <div className="border-b border-line px-3 py-2 font-semibold">{t("PSQL lines, the costliest first")}</div>
          <table className="w-full border-collapse">
            <thead className="text-left text-muted">
              <tr>
                <th className="px-2 py-1 font-normal">{t("Line")}</th>
                <th className="px-2 py-1 text-right font-normal">{t("Times")}</th>
                <th className="px-2 py-1 text-right font-normal">{t("Time")}</th>
                <th className="px-2 py-1 text-right font-normal">{t("Each")}</th>
                <th className="px-2 py-1 text-right font-normal">{t("Slowest")}</th>
              </tr>
            </thead>
            <tbody>
              {lines.map((l) => (
                <tr key={`${l.statement}-${l.line}-${l.column}`}>
                  <td className={`${cell} tabular-nums`}>
                    {t("line {line}, column {column}", { line: l.line, column: l.column })}
                  </td>
                  <td className={`${cell} text-right tabular-nums`}>{n(l.count)}</td>
                  <td className={`${cell} text-right tabular-nums`}>{time(l.total)}</td>
                  <td className={`${cell} text-right tabular-nums`}>{time(l.total / Math.max(1, l.count))}</td>
                  <td className={`${cell} text-right tabular-nums`}>{time(l.max)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </>
  );
}
