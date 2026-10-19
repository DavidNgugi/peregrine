import type { ReactElement } from "react";

const stroke = {
  fill: "none",
  stroke: "currentColor",
  strokeLinecap: "round" as const,
  strokeLinejoin: "round" as const,
  strokeWidth: 1.8,
  viewBox: "0 0 24 24",
  "aria-hidden": true,
};

export function HomeIcon(): ReactElement {
  return (
    <svg {...stroke}>
      <path d="M3 10.5 12 3l9 7.5V21a1 1 0 0 1-1 1h-5v-7H9v7H4a1 1 0 0 1-1-1z" />
    </svg>
  );
}

export function CompassIcon(): ReactElement {
  return (
    <svg {...stroke}>
      <circle cx="12" cy="12" r="9" />
      <path d="m15.5 8.5-2.1 4.9-4.9 2.1 2.1-4.9z" />
    </svg>
  );
}

export function BookmarkIcon(): ReactElement {
  return (
    <svg {...stroke}>
      <path d="M6 4h12v17l-6-4-6 4z" />
    </svg>
  );
}

export function LibraryIcon(): ReactElement {
  return (
    <svg {...stroke}>
      <rect height="7" rx="1" width="7" x="3" y="3" />
      <rect height="7" rx="1" width="7" x="14" y="3" />
      <rect height="7" rx="1" width="7" x="3" y="14" />
      <rect height="7" rx="1" width="7" x="14" y="14" />
    </svg>
  );
}

export function SearchIcon(): ReactElement {
  return (
    <svg {...stroke}>
      <circle cx="11" cy="11" r="7" />
      <path d="m20 20-3.6-3.6" />
    </svg>
  );
}

export function ChevronLeftIcon(): ReactElement {
  return (
    <svg {...stroke} strokeWidth={2}>
      <path d="m15 5-7 7 7 7" />
    </svg>
  );
}
