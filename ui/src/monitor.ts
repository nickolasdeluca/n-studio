// Monitoring sessions: a trace of everything a server runs, recorded by
// the backend from start to stop. The backend keeps the events and what
// they add up to; here is only where each session stands and the seconds
// its charts draw.
import { listen } from "@tauri-apps/api/event";
import { create } from "zustand";
import { api, errorText, type TraceOptions, type TraceSecond, type TraceTick } from "./api";

/** Seconds kept for the charts: what the backend keeps */
const SECONDS = 3600;

export interface MonitorSession {
  run: number;
  /** Where it stands, as last told */
  tick: TraceTick;
  seconds: TraceSecond[];
  /** When it started here; a recorded session opened from its log has none */
  started?: number;
  ended?: number;
}

interface MonitorState {
  /** By the activity monitor's panel id: one session at a time in each */
  sessions: Record<string, MonitorSession>;
  errors: Record<string, string>;
}

export const useMonitor = create<MonitorState>(() => ({ sessions: {}, errors: {} }));

/** `more` over `seconds`: the last second told may have grown since */
function merge(seconds: TraceSecond[], more: TraceSecond[]): TraceSecond[] {
  if (more.length === 0) return seconds;
  const from = more[0].at;
  return [...seconds.filter((s) => s.at < from), ...more].slice(-SECONDS);
}

function take(tick: TraceTick) {
  useMonitor.setState((s) => {
    const entry = Object.entries(s.sessions).find(([, session]) => session.run === tick.run);
    if (!entry) return s;
    const [panel, session] = entry;
    const over = tick.standing !== "recording" && session.tick.standing === "recording";
    return {
      sessions: {
        ...s.sessions,
        [panel]: {
          ...session,
          tick: { ...tick, seconds: [] },
          seconds: merge(session.seconds, tick.seconds),
          ended: over ? Date.now() : session.ended,
        },
      },
    };
  });
}

let subscribed: Promise<boolean> | null = null;

/** Whether the backend's events arrive: outside Tauri they do not */
function listening(): Promise<boolean> {
  subscribed ??= listen<TraceTick>("trace-tick", (event) => take(event.payload)).then(
    () => true,
    () => false,
  );
  return subscribed;
}

function setError(panel: string, error?: string) {
  useMonitor.setState((s) => {
    const errors = { ...s.errors };
    if (error) errors[panel] = error;
    else delete errors[panel];
    return { errors };
  });
}

let runs = 0;
const newRun = () => Date.now() * 1000 + (runs++ % 1000);

function forget(panel: string) {
  const session = useMonitor.getState().sessions[panel];
  if (session) void api.traceClose(session.run).catch(() => {});
}

function put(panel: string, tick: TraceTick, started?: number) {
  useMonitor.setState((s) => ({
    sessions: { ...s.sessions, [panel]: { run: tick.run, tick: { ...tick, seconds: [] }, seconds: tick.seconds, started } },
  }));
}

/** Starts recording in a panel, in place of whatever session it showed */
export async function startMonitor(panel: string, registration: string, options: TraceOptions) {
  setError(panel);
  try {
    const events = await listening();
    const run = newRun();
    const tick = await api.traceStart(registration, run, options);
    forget(panel);
    put(panel, tick, Date.now());
    if (!events) {
      // No events to be told by: ask
      const timer = setInterval(() => {
        const session = useMonitor.getState().sessions[panel];
        if (session?.run !== run || session.tick.standing !== "recording") return clearInterval(timer);
        void api.traceState(run).then(take, () => clearInterval(timer));
      }, 1000);
    }
  } catch (e) {
    setError(panel, errorText(e));
  }
}

export function stopMonitor(panel: string) {
  const session = useMonitor.getState().sessions[panel];
  if (session?.tick.standing === "recording") void api.traceStop(session.run);
}

/** Opens a recorded session's log in a panel */
export async function openRecorded(panel: string, path: string) {
  setError(panel);
  try {
    const tick = await api.traceOpen(newRun(), path);
    forget(panel);
    put(panel, tick);
  } catch (e) {
    setError(panel, errorText(e));
  }
}

/** The panel closed: the session stops and what it kept is let go. Its
 *  log stays. */
export function closeMonitor(panel: string) {
  forget(panel);
  useMonitor.setState((s) => {
    const sessions = { ...s.sessions };
    const errors = { ...s.errors };
    delete sessions[panel];
    delete errors[panel];
    return { sessions, errors };
  });
}
