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

export function IntegrationsIcon(): ReactElement {
  return (
    <svg {...stroke}>
      <path d="M4 7h10M18 7h2M4 17h2M10 17h10" />
      <circle cx="16" cy="7" r="2" />
      <circle cx="8" cy="17" r="2" />
    </svg>
  );
}

export function DownloadIcon(): ReactElement {
  return (
    <svg {...stroke}>
      <path d="M12 3v12" />
      <path d="m7 10 5 5 5-5" />
      <path d="M4 21h16" />
    </svg>
  );
}

export function StarIcon(): ReactElement {
  return (
    <svg {...stroke} fill="currentColor" stroke="none">
      <path d="m12 3 2.8 5.7 6.2.9-4.5 4.4 1.1 6.2L12 17.3l-5.5 2.9 1.1-6.2L3 9.6l6.2-.9z" />
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

export function PlayIcon(): ReactElement {
  return (
    <svg {...stroke} fill="currentColor" stroke="none">
      <path d="M7 4.5v15a.8.8 0 0 0 1.2.7l12.2-7.5a.8.8 0 0 0 0-1.4L8.2 3.8A.8.8 0 0 0 7 4.5z" />
    </svg>
  );
}

export function PauseIcon(): ReactElement {
  return (
    <svg {...stroke} fill="currentColor" stroke="none">
      <rect height="16" rx="1.2" width="4.5" x="5.5" y="4" />
      <rect height="16" rx="1.2" width="4.5" x="14" y="4" />
    </svg>
  );
}

export function RewindIcon(): ReactElement {
  return (
    <svg {...stroke}>
      <path d="M4 12a8 8 0 1 0 2.4-5.7" />
      <path d="M4 4v4h4" />
    </svg>
  );
}

export function ForwardIcon(): ReactElement {
  return (
    <svg {...stroke}>
      <path d="M20 12a8 8 0 1 1-2.4-5.7" />
      <path d="M20 4v4h-4" />
    </svg>
  );
}

export function SubtitlesIcon(): ReactElement {
  return (
    <svg {...stroke}>
      <rect height="14" rx="2" width="18" x="3" y="5" />
      <path d="M7 13h4M13 13h4M7 16h7" />
    </svg>
  );
}

export function NextIcon(): ReactElement {
  return (
    <svg {...stroke} fill="currentColor" stroke="none">
      <path d="M5 5.2v13.6a.7.7 0 0 0 1.1.6l9.8-6.8a.7.7 0 0 0 0-1.2L6.1 4.6a.7.7 0 0 0-1.1.6z" />
      <rect height="14" rx="1" width="3" x="16.5" y="5" />
    </svg>
  );
}

export function FullscreenIcon(): ReactElement {
  return (
    <svg {...stroke}>
      <path d="M4 9V4h5M20 9V4h-5M4 15v5h5M20 15v5h-5" />
    </svg>
  );
}

export function CheckIcon(): ReactElement {
  return (
    <svg {...stroke} strokeWidth={2.2}>
      <path d="m5 12.5 4.5 4.5L19 7.5" />
    </svg>
  );
}
