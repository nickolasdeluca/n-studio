import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { create } from "zustand";
import { Icon } from "../icons";

export interface MenuItem {
  label: string;
  icon?: string;
  disabled?: boolean;
  danger?: boolean;
  onSelect: () => void;
}

type Entry = MenuItem | "separator";

interface MenuState {
  open: { x: number; y: number; items: Entry[] } | null;
  show(x: number, y: number, items: Entry[]): void;
  close(): void;
}

export const useContextMenu = create<MenuState>()((set) => ({
  open: null,
  show: (x, y, items) => set({ open: { x, y, items } }),
  close: () => set({ open: null }),
}));

/** The one context menu, rendered at the root and kept inside the window */
export function ContextMenuHost() {
  const { open, close } = useContextMenu();
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState({ x: 0, y: 0 });
  const [active, setActive] = useState(-1);

  useLayoutEffect(() => {
    if (!open || !ref.current) return;
    const { width, height } = ref.current.getBoundingClientRect();
    setPos({
      x: Math.min(open.x, window.innerWidth - width - 4),
      y: Math.min(open.y, window.innerHeight - height - 4),
    });
    setActive(-1);
    ref.current.focus();
  }, [open]);

  useEffect(() => {
    if (!open) return;
    const dismiss = () => close();
    window.addEventListener("blur", dismiss);
    window.addEventListener("resize", dismiss);
    return () => {
      window.removeEventListener("blur", dismiss);
      window.removeEventListener("resize", dismiss);
    };
  }, [open, close]);

  if (!open) return null;
  const items = open.items;
  const selectable = items.flatMap((e, i) => (e !== "separator" && !e.disabled ? [i] : []));

  const choose = (item: MenuItem) => {
    close();
    item.onSelect();
  };

  return (
    <div className="fixed inset-0 z-50" onMouseDown={close} onContextMenu={(e) => (e.preventDefault(), close())}>
      <div
        ref={ref}
        role="menu"
        tabIndex={-1}
        className="absolute min-w-48 rounded-md border border-line bg-panel py-1 shadow-[var(--shadow)] outline-none"
        style={{ left: pos.x, top: pos.y }}
        onMouseDown={(e) => e.stopPropagation()}
        onKeyDown={(e) => {
          if (e.key === "Escape") close();
          else if (e.key === "ArrowDown" || e.key === "ArrowUp") {
            e.preventDefault();
            const at = selectable.indexOf(active);
            const next = e.key === "ArrowDown" ? at + 1 : at <= 0 ? selectable.length - 1 : at - 1;
            setActive(selectable[next % selectable.length] ?? -1);
          } else if (e.key === "Enter" && active >= 0) {
            const item = items[active];
            if (item !== "separator") choose(item);
          }
        }}
      >
        {items.map((item, i) =>
          item === "separator" ? (
            <div key={i} className="my-1 h-px bg-line" />
          ) : (
            <button
              key={i}
              role="menuitem"
              disabled={item.disabled}
              onMouseEnter={() => setActive(i)}
              onClick={() => choose(item)}
              className={`flex h-6 w-full items-center gap-2 px-3 text-left disabled:opacity-40 ${
                active === i && !item.disabled ? "bg-accent text-accent-fg" : item.danger ? "text-danger" : ""
              }`}
            >
              <span className="w-3.5">{item.icon && <Icon name={item.icon} size={13} />}</span>
              {item.label}
            </button>
          ),
        )}
      </div>
    </div>
  );
}
