// Claude Code, in a terminal: the folder it works in, starting and stopping
// it, and what to do when it is not installed.
import { open } from "@tauri-apps/plugin-dialog";
import "@xterm/xterm/css/xterm.css";
import { useEffect, useRef } from "react";
import { attachTerminal, fitTerminal, listening, useAgent } from "../agent";
import { baseName } from "../dock";
import { t } from "../i18n";
import { Icon } from "../icons";

/** What the shell says when it finds no `claude` */
const NOT_FOUND = 127;

export function AgentPanel() {
  const host = useRef<HTMLDivElement>(null);
  const { status, folder, error, exitCode } = useAgent();
  const running = status === "running" || status === "starting";

  useEffect(() => {
    listening();
    const el = host.current;
    if (!el) return;
    attachTerminal(el);
    const observer = new ResizeObserver(() => fitTerminal());
    observer.observe(el);
    return () => observer.disconnect();
  }, []);

  const pick = async () => {
    const picked = await open({ title: t("The Project Folder Claude Code Works In"), directory: true, defaultPath: folder ?? undefined });
    if (typeof picked === "string") useAgent.getState().setFolder(picked);
  };

  return (
    <div className="flex h-full flex-col bg-panel">
      <div className="flex h-8 shrink-0 items-center gap-1 border-b border-line px-1.5 text-muted">
        <button
          type="button"
          onClick={pick}
          disabled={running}
          title={folder ?? t("No project: Claude Code works with the databases only. Choose a folder to give it a project's files too.")}
          className="flex h-6 min-w-0 items-center gap-1.5 rounded px-1.5 text-fg hover:bg-hover disabled:text-muted disabled:hover:bg-transparent"
        >
          <Icon name="folder" size={13} />
          <span className="truncate">{folder ? baseName(folder) : t("No project (databases only)")}</span>
        </button>
        {folder && !running && (
          <button
            type="button"
            onClick={() => useAgent.getState().setFolder(null)}
            title={t("No project: the databases only")}
            aria-label={t("No project: the databases only")}
            className="grid size-6 shrink-0 place-items-center rounded text-muted hover:bg-hover hover:text-fg"
          >
            ×
          </button>
        )}
        <span className="flex-1" />
        {running ? (
          <button
            type="button"
            onClick={() => void useAgent.getState().stop()}
            className="flex h-6 items-center gap-1.5 rounded px-2 text-fg hover:bg-hover"
          >
            <Icon name="stop" size={12} className="text-danger" />
            {t("Stop")}
          </button>
        ) : (
          <button
            type="button"
            onClick={() => void useAgent.getState().start()}
            className="flex h-6 items-center gap-1.5 rounded px-2 text-fg hover:bg-hover disabled:text-faint disabled:hover:bg-transparent"
          >
            <Icon name="run" size={12} className="text-ok" />
            {status === "ended" ? t("Start Again") : t("Start Claude Code")}
          </button>
        )}
      </div>
      {!running && (
        <div className="shrink-0 border-b border-line px-3 py-2 text-[12px] leading-relaxed text-muted">
          {error ? (
            <p className="text-danger">{error}</p>
          ) : exitCode === NOT_FOUND ? (
            <p>
              <span className="text-danger">{t("Claude Code was not found.")} </span>
              {t("Install it from claude.com/claude-code, sign in once in a terminal with `claude`, then start it here again.")}
            </p>
          ) : status === "stopped" ? (
            <p>
              {t(
                "Claude Code runs here with your own sign-in: with the databases only, or in a project folder you choose. It can read the metadata of the connected databases and run SELECTs whose rows only you see; rows reach it only when you share them from a grid.",
              )}
            </p>
          ) : null}
        </div>
      )}
      <div ref={host} className="agent-terminal min-h-0 flex-1 overflow-hidden px-1.5 pt-1" />
    </div>
  );
}
