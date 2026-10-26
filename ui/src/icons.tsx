// Object icons: our own small set, one per kind, following IBExpert's icon
// grammar (a distinct glyph and color per object type) without its artwork.
import type { ReactNode } from "react";

interface IconProps {
  className?: string;
  size?: number;
}

function Svg({ children, className, size = 14 }: IconProps & { children: ReactNode }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.3}
      strokeLinecap="round"
      strokeLinejoin="round"
      className={className}
      aria-hidden
    >
      {children}
    </svg>
  );
}

const glyphs: Record<string, ReactNode> = {
  database: (
    <>
      <ellipse cx="8" cy="3.8" rx="5" ry="1.9" />
      <path d="M3 3.8v8.4c0 1 2.2 1.9 5 1.9s5-.9 5-1.9V3.8M3 8c0 1 2.2 1.9 5 1.9s5-.9 5-1.9" />
    </>
  ),
  domain: <path d="M8 2.2 13 5v6L8 13.8 3 11V5zM3 5l5 2.8L13 5M8 7.8v6" />,
  table: (
    <>
      <rect x="2.2" y="3" width="11.6" height="10" rx="1" />
      <path d="M2.2 6.3h11.6M2.2 9.6h11.6M6.3 6.3V13" />
    </>
  ),
  view: (
    <>
      <path d="M1.8 8s2.3-4.2 6.2-4.2S14.2 8 14.2 8s-2.3 4.2-6.2 4.2S1.8 8 1.8 8z" />
      <circle cx="8" cy="8" r="1.9" />
    </>
  ),
  procedure: (
    <>
      <rect x="2.2" y="2.2" width="11.6" height="11.6" rx="1.5" />
      <path d="m5.2 6 2 2-2 2M8.6 10.6h2.4" />
    </>
  ),
  function: <path d="M10.8 2.6c-1.6-.4-2.7.3-3 1.9L6.4 11.6c-.3 1.6-1.4 2.2-3 1.8M4.6 6.4h5.6" />,
  package: (
    <>
      <path d="M2.4 4.6 8 2l5.6 2.6v6.8L8 14l-5.6-2.6z" />
      <path d="M2.4 4.6 8 7.2l5.6-2.6M8 7.2V14M5.2 3.3l5.6 2.6" />
    </>
  ),
  trigger: <path d="M9 1.8 3.6 9h4l-1 5.2L12.4 7h-4z" />,
  generator: (
    <>
      <path d="M3 5.4h10M3 10.6h10M6.2 2.6 5.2 13.4M10.8 2.6l-1 10.8" />
    </>
  ),
  exception: (
    <>
      <path d="M8 2.2 14 13H2z" />
      <path d="M8 6.6v3M8 11.3v.1" />
    </>
  ),
  udf: (
    <>
      <path d="M6 2.6H4.4c-.8 0-1.2.4-1.2 1.2v2.6L2 8l1.2 1.6v2.6c0 .8.4 1.2 1.2 1.2H6M10 2.6h1.6c.8 0 1.2.4 1.2 1.2v2.6L14 8l-1.2 1.6v2.6c0 .8-.4 1.2-1.2 1.2H10" />
    </>
  ),
  role: (
    <>
      <circle cx="8" cy="5.4" r="2.6" />
      <path d="M3 13.6c.5-2.6 2.5-4 5-4s4.5 1.4 5 4" />
    </>
  ),
  index: (
    <>
      <path d="M3 3.4h6.4M3 6.4h4.4M3 9.4h6.4M3 12.4h3.4" />
      <path d="M12 3v10M10.2 11.2 12 13l1.8-1.8" />
    </>
  ),
  chevron: <path d="m6 4 4 4-4 4" />,
  plus: <path d="M8 3v10M3 8h10" />,
  plug: (
    <>
      <path d="M6 2v3M10 2v3M4.2 5h7.6v2.4a3.8 3.8 0 0 1-7.6 0zM8 11.2V14" />
    </>
  ),
  unplug: (
    <>
      <path d="M6 2v3M10 2v3M4.2 5h7.6v2.4a3.8 3.8 0 0 1-7.6 0zM8 11.2V14M2.4 2.4l11.2 11.2" />
    </>
  ),
  refresh: (
    <>
      <path d="M13 3.4v3.2H9.8" />
      <path d="M12.6 6.6A5 5 0 1 0 13 9.4" />
    </>
  ),
  edit: <path d="m10.4 2.8 2.8 2.8-7.4 7.4H3v-2.8zM8.8 4.4l2.8 2.8" />,
  trash: <path d="M3 4.4h10M6.4 4.4V2.8h3.2v1.6M4.4 4.4l.6 8.8h6l.6-8.8" />,
  search: (
    <>
      <circle cx="7" cy="7" r="4.2" />
      <path d="m10.2 10.2 3.4 3.4" />
    </>
  ),
  sql: <path d="M3.5 2h6l3 3v9h-9zM9.5 2v3h3M5.5 8h5M5.5 10.5h3.5" />,
  run: <path d="M5 3.2v9.6L12.6 8z" fill="currentColor" />,
  "run-all": <path d="M2.8 3.6v8.8L8 8zM8 3.6v8.8L13.2 8z" fill="currentColor" />,
  "run-cursor": (
    <>
      <path d="M3.5 3.6v8.8L9 8z" fill="currentColor" />
      <path d="M12 3.4v9.2" />
    </>
  ),
  stop: <rect x="4" y="4" width="8" height="8" rx="1" fill="currentColor" />,
  commit: <path d="m3 8.4 3.2 3.2L13 4.8" />,
  rollback: (
    <>
      <path d="M3.4 6.4h6.4a3.2 3.2 0 0 1 0 6.4H6.4" />
      <path d="m5.8 4 -2.4 2.4 2.4 2.4" />
    </>
  ),
  first: <path d="M4 3.5v9M12 3.5 7 8l5 4.5" />,
  prev: <path d="M10.5 3.5 5.5 8l5 4.5" />,
  next: <path d="M5.5 3.5 10.5 8l-5 4.5" />,
  last: <path d="M12 3.5v9M4 3.5 9 8l-5 4.5" />,
  "fetch-all": <path d="M8 2.5v8M4.5 7 8 10.5 11.5 7M3.5 13.5h9" />,
  panel: (
    <>
      <rect x="2.2" y="2.6" width="11.6" height="10.8" rx="1" />
      <path d="M6.2 2.6v10.8" />
    </>
  ),
};

/** Per-kind color, so a long mixed list can be scanned by color alone */
export const kindColor: Record<string, string> = {
  database: "text-[#d08a1e]",
  domain: "text-[#8a63d2]",
  table: "text-[#2f7fd8]",
  view: "text-[#1f9c8c]",
  procedure: "text-[#c25b2a]",
  function: "text-[#b9479a]",
  package: "text-[#7a6a32]",
  trigger: "text-[#d4a514]",
  generator: "text-[#4d8a3a]",
  exception: "text-[#cc3d3d]",
  udf: "text-[#7c5cc4]",
  role: "text-[#3b7f9e]",
  index: "text-[#64748b]",
};

export function Icon({ name, ...props }: IconProps & { name: string }) {
  return <Svg {...props}>{glyphs[name] ?? glyphs.table}</Svg>;
}
