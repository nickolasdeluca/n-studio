import { useEffect, useRef, type FormEvent, type ReactNode } from "react";

/** A dialog over the app: Escape cancels, Enter submits, focus stays inside */
export function Modal(props: {
  title: string;
  width?: string;
  onCancel: () => void;
  onSubmit: () => void;
  children: ReactNode;
  footer: ReactNode;
}) {
  const ref = useRef<HTMLFormElement>(null);

  useEffect(() => {
    // Focus goes back where it was (the explorer, usually) when the dialog closes
    const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    // One selector list would match in document order, not in this order
    const first =
      ref.current?.querySelector<HTMLElement>("[data-autofocus]") ??
      ref.current?.querySelector<HTMLElement>("input, select, button");
    first?.focus();
    if (first instanceof HTMLInputElement) first.select();
    return () => previous?.focus();
  }, []);

  const submit = (e: FormEvent) => {
    e.preventDefault();
    props.onSubmit();
  };

  return (
    <div className="fixed inset-0 z-40 grid place-items-center bg-black/25 p-4" onMouseDown={props.onCancel}>
      <form
        ref={ref}
        role="dialog"
        aria-modal
        aria-label={props.title}
        onSubmit={submit}
        onMouseDown={(e) => e.stopPropagation()}
        onKeyDown={(e) => {
          if (e.key === "Escape") {
            e.stopPropagation();
            props.onCancel();
          } else if (e.key === "Tab") trapFocus(e, ref.current);
        }}
        className={`flex max-h-full flex-col rounded-lg border border-line bg-panel shadow-[var(--shadow)] ${props.width ?? "w-[30rem]"}`}
      >
        <h2 className="border-b border-line px-4 py-2.5 text-[13px] font-semibold">{props.title}</h2>
        <div className="scroll-thin overflow-auto px-4 py-3">{props.children}</div>
        <div className="flex items-center gap-2 border-t border-line bg-panel-2 px-4 py-2.5">{props.footer}</div>
      </form>
    </div>
  );
}

function trapFocus(e: React.KeyboardEvent, root: HTMLElement | null) {
  if (!root) return;
  const items = [...root.querySelectorAll<HTMLElement>("input, select, button, textarea, [tabindex]")].filter(
    (el) => !el.hasAttribute("disabled") && el.tabIndex >= 0,
  );
  const first = items[0];
  const last = items[items.length - 1];
  if (e.shiftKey && document.activeElement === first) {
    e.preventDefault();
    last?.focus();
  } else if (!e.shiftKey && document.activeElement === last) {
    e.preventDefault();
    first?.focus();
  }
}

export function Button(props: {
  children: ReactNode;
  onClick?: () => void;
  primary?: boolean;
  danger?: boolean;
  disabled?: boolean;
  submit?: boolean;
}) {
  const tone = props.primary
    ? "border-accent bg-accent text-accent-fg hover:brightness-110"
    : props.danger
      ? "border-danger bg-danger text-white hover:brightness-110"
      : "border-line bg-panel hover:bg-hover";
  return (
    <button
      type={props.submit ? "submit" : "button"}
      onClick={props.onClick}
      disabled={props.disabled}
      className={`h-7 rounded border px-3 disabled:opacity-50 ${tone}`}
    >
      {props.children}
    </button>
  );
}

export function Field(props: { label: string; children: ReactNode; hint?: string; className?: string }) {
  return (
    <label className={`flex flex-col gap-1 ${props.className ?? ""}`}>
      <span className="text-muted">{props.label}</span>
      {props.children}
      {props.hint && <span className="text-[11.5px] text-faint">{props.hint}</span>}
    </label>
  );
}

export const inputClass =
  "h-7 rounded border border-line bg-bg px-2 text-fg outline-none focus:border-accent placeholder:text-faint";
