// The preferences: what this machine's user changed of the app. Each change
// is in use as soon as it is made, and kept; there is nothing to save.
import { useState } from "react";
import { t } from "../i18n";
import { COMMANDS, conflicts, display, keysOf, normalize, pressed, SCOPES, type Command } from "../keymap";
import { isMac } from "../keys";
import { usePrefs } from "../prefs";
import { useStore } from "../store";
import { Button, inputClass, Modal } from "./Modal";

type Section = "keys";

const sections: [Section, string][] = [["keys", "Keyboard"]];

export function PreferencesDialog() {
  const [section, setSection] = useState<Section>("keys");
  const close = () => useStore.getState().openDialog(null);
  return (
    <Modal
      title={t("Preferences")}
      width="w-[52rem]"
      onCancel={close}
      onSubmit={close}
      footer={
        <>
          <span className="min-w-0 flex-1 truncate px-1 text-muted">{t("Changes are in use as soon as they are made.")}</span>
          <Button primary submit>
            {t("Close")}
          </Button>
        </>
      }
    >
      <div className="flex h-[30rem] gap-4">
        <nav className="flex w-36 shrink-0 flex-col gap-0.5" aria-label={t("Sections")}>
          {sections.map(([id, label]) => (
            <button
              key={id}
              type="button"
              onClick={() => setSection(id)}
              className={`h-7 rounded px-2 text-left ${section === id ? "bg-selected" : "hover:bg-hover"}`}
            >
              {t(label)}
            </button>
          ))}
        </nav>
        <div className="flex min-w-0 flex-1 flex-col">{section === "keys" && <KeysPane />}</div>
      </div>
    </Modal>
  );
}

/** Whether a command or one of its keys is what the filter asks for */
function found(c: Command, keys: string[], filter: string): boolean {
  const f = filter.trim().toLowerCase();
  if (!f) return true;
  return (
    t(c.label).toLowerCase().includes(f) ||
    c.label.toLowerCase().includes(f) ||
    keys.some((k) => display(k, isMac).toLowerCase().includes(f) || k.toLowerCase().includes(f))
  );
}

function KeysPane() {
  const overrides = usePrefs((s) => s.keys);
  const { setKeys, resetKeys } = usePrefs.getState();
  const [filter, setFilter] = useState("");
  /** The command a key is being taken for */
  const [taking, setTaking] = useState<string | null>(null);
  const changed = Object.keys(overrides).length > 0;

  return (
    <>
      <div className="mb-2 flex items-center gap-2">
        <input
          className={`${inputClass} min-w-0 flex-1`}
          placeholder={t("Find a command or a key")}
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          data-autofocus
        />
        <Button disabled={!changed} onClick={resetKeys}>
          {t("Reset All")}
        </Button>
      </div>
      <div className="scroll-thin min-h-0 flex-1 overflow-auto rounded border border-line">
        {SCOPES.map(([scope, label]) => {
          const commands = COMMANDS.filter((c) => c.scope === scope && found(c, keysOf(c.id, overrides), filter));
          if (commands.length === 0) return null;
          return (
            <section key={scope}>
              <h3 className="sticky top-0 z-10 border-b border-line bg-panel-2 px-2 py-1 font-semibold text-muted">
                {t(label)}
              </h3>
              {commands.map((c) => (
                <CommandRow
                  key={c.id}
                  command={c}
                  taking={taking === c.id}
                  onTake={(on) => setTaking(on ? c.id : null)}
                  overrides={overrides}
                  setKeys={setKeys}
                />
              ))}
            </section>
          );
        })}
      </div>
      <p className="mt-2 text-[11.5px] text-faint">
        {t(
          "Add takes the next key pressed; Esc gives up. The keys that move about and edit text stay as they are.",
        )}
      </p>
    </>
  );
}

function CommandRow(props: {
  command: Command;
  taking: boolean;
  onTake(on: boolean): void;
  overrides: Record<string, string[]>;
  setKeys(id: string, keys: string[] | null): void;
}) {
  const { command: c, taking, onTake, overrides, setKeys } = props;
  const keys = keysOf(c.id, overrides);
  const own = c.id in overrides;
  // One of each: Mod-S and Ctrl-S are one key but on a Mac
  const shown = keys.filter((k, i) => keys.findIndex((x) => normalize(x, isMac) === normalize(k, isMac)) === i);
  const same = (a: string) => (b: string) => normalize(a, isMac) === normalize(b, isMac);

  const take = (e: React.KeyboardEvent) => {
    // Tab still moves on, and Esc gives up
    if ((e.key === "Tab" && !e.ctrlKey && !e.altKey && !e.metaKey) || e.key === "Escape") {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        onTake(false);
      }
      return;
    }
    e.preventDefault();
    e.stopPropagation();
    const key = pressed(e);
    // A modifier on its own: the key it goes with is still to come
    if (!key) return;
    if (!keys.some((k) => normalize(k, isMac) === key)) setKeys(c.id, [...keys, key]);
    onTake(false);
  };

  return (
    <div className="flex min-h-8 items-center gap-2 border-b border-line/60 px-2 py-1 last:border-b-0 hover:bg-hover/50">
      <span className="w-48 shrink-0 truncate">{t(c.label)}</span>
      <div className="flex min-w-0 flex-1 flex-wrap items-center gap-1">
        {shown.map((k) => {
          const also = conflicts(c.id, k, overrides, isMac);
          const clash = also.length > 0;
          return (
            <span
              key={k}
              title={clash ? t("Also: {commands}", { commands: also.map((o) => t(o.label)).join(", ") }) : undefined}
              className={`inline-flex h-6 items-center gap-1 rounded border pl-1.5 font-mono text-[11.5px] ${
                clash ? "border-warn text-warn" : "border-line bg-panel-2"
              }`}
            >
              {display(k, isMac)}
              <button
                type="button"
                aria-label={t("Remove {key}", { key: display(k, isMac) })}
                onClick={() => setKeys(c.id, keys.filter((x) => !same(k)(x)))}
                className="grid h-full w-5 place-items-center rounded-r text-muted hover:bg-hover hover:text-fg"
              >
                ×
              </button>
            </span>
          );
        })}
        {keys.length === 0 && !taking && <span className="text-faint">{t("No key")}</span>}
        {taking && (
          <button
            type="button"
            autoFocus
            onKeyDown={take}
            onBlur={() => onTake(false)}
            className="h-6 rounded border border-dashed border-accent px-2 text-[11.5px] text-accent"
          >
            {t("Press the keys…")}
          </button>
        )}
      </div>
      <div className="flex shrink-0 items-center gap-1">
        <button
          type="button"
          onClick={() => onTake(true)}
          className="h-6 rounded px-2 text-muted hover:bg-hover hover:text-fg"
        >
          {t("Add")}
        </button>
        <button
          type="button"
          disabled={!own}
          title={t("Back to {keys}", { keys: [...new Set(c.keys.map((k) => display(k, isMac)))].join(", ") || t("no key") })}
          onClick={() => setKeys(c.id, null)}
          className="h-6 rounded px-2 text-muted hover:bg-hover hover:text-fg disabled:invisible"
        >
          {t("Reset")}
        </button>
      </div>
    </div>
  );
}
