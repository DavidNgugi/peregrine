import { useEffect, useState } from "react";

import { api, type Download, type Tile } from "./api.js";
import { useAutoFocus } from "./focus.js";
import { formatBytes as bytes } from "./format.js";

function percent(download: Download): number {
  return Math.max(0, Math.min(100, Math.round(download.progressPermille / 10)));
}

function rate(value: number): string {
  if (value <= 0) return "";
  return bytes(value) + "/s";
}

function eta(seconds: number | null): string {
  if (seconds === null || seconds <= 0) return "";
  const minutes = Math.ceil(seconds / 60);
  return minutes < 60 ? String(minutes) + " min left" : String(Math.ceil(minutes / 60)) + " h left";
}

type StatusTone = "queued" | "info" | "active" | "warn" | "done" | "danger";

/** The colour a row's state badge wears; the label still carries the state in words. */
function statusTone(state: string): StatusTone {
  switch (state) {
    case "inspecting":
    case "metadata":
      return "info";
    case "downloading":
      return "active";
    case "paused":
      return "warn";
    case "seeding":
    case "completed":
      return "done";
    case "failed":
    case "blocked":
      return "danger";
    default:
      return "queued";
  }
}

function label(state: string): string {
  switch (state) {
    case "downloading":
      return "Downloading";
    case "queued":
      return "Queued";
    case "inspecting":
    case "metadata":
      return "Preparing";
    case "paused":
      return "Paused";
    case "seeding":
      return "Seeding";
    case "completed":
      return "Downloaded";
    case "failed":
      return "Failed";
    case "blocked":
      return "Blocked";
    default:
      return state;
  }
}

function tileOf(download: Download): Tile | null {
  if (download.title === null) return null;
  return {
    id: download.titleId,
    name: download.title,
    kind: download.kind ?? "movie",
    year: null,
    poster: download.poster,
    backdrop: null,
    rating: null,
    overview: null,
    playable: true,
    percent: null,
    positionMs: 0,
    watchlisted: false,
  };
}

/** The desktop's download queue, as progress only: starting a transfer stays in the desktop app. */
export function DownloadsScreen({ onOpen }: { readonly onOpen: (tile: Tile) => void }) {
  const [rows, setRows] = useState<Download[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  useAutoFocus(rows[0]?.id);

  useEffect(() => {
    let cancelled = false;
    const read = async (): Promise<void> => {
      try {
        const found = await api.downloads();
        if (cancelled) return;
        setRows(found);
        setError(null);
      } catch (caught) {
        if (!cancelled) {
          setError(caught instanceof Error ? caught.message : "Downloads could not be read.");
        }
      } finally {
        if (!cancelled) setLoading(false);
      }
    };
    void read();
    const timer = window.setInterval(() => void read(), 2000);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, []);

  return (
    <main className="screen downloads-screen">
      <header className="browse-head">
        <div>
          <h1>Downloads</h1>
        </div>
        <p className="dim">{rows.length === 1 ? "1 transfer" : rows.length + " transfers"}</p>
      </header>
      <p className="browse-note">
        Transfers start on the desktop app; their progress is live here.
      </p>
      {error === null ? null : <p className="error">{error}</p>}
      {loading ? <p className="dim browse-loading">Reading...</p> : null}
      {!loading && rows.length === 0 ? (
        <p className="dim browse-empty">Nothing is downloading right now.</p>
      ) : (
        <div className="downloads-list">
          {rows.map((download, index) => {
            const tile = tileOf(download);
            return (
              <button
                className="download-row"
                data-autofocus={index === 0 ? "true" : undefined}
                data-focus="true"
                data-tone={statusTone(download.state)}
                disabled={tile === null}
                key={download.id}
                onClick={() => {
                  if (tile !== null) onOpen(tile);
                }}
                type="button"
              >
                <span className="download-poster">
                  {download.poster === null ? (
                    <span className="card-placeholder" />
                  ) : (
                    <img alt="" decoding="async" loading="lazy" src={download.poster} />
                  )}
                </span>
                <span className="download-copy">
                  <span className="download-title">
                    {download.title ?? "Unknown title"}
                    {download.seasonNumber === null ? null : (
                      <span className="download-episode">
                        {"S" +
                          String(download.seasonNumber) +
                          " E" +
                          String(download.episodeNumber ?? 0)}
                      </span>
                    )}
                  </span>
                  <span className="download-state-line">
                    <span className={`download-state-badge tone-${statusTone(download.state)}`}>
                      {label(download.state)}
                    </span>
                    {download.state === "downloading" || download.state === "paused" ? (
                      <span className="download-percent">{percent(download)}%</span>
                    ) : null}
                  </span>
                  <span className="download-bar">
                    <i style={{ width: percent(download) + "%" }} />
                  </span>
                  <span className="download-meta">
                    {[
                      bytes(download.downloadedBytes) +
                        (download.totalBytes === null ? "" : " of " + bytes(download.totalBytes)),
                      rate(download.rateBps),
                      eta(download.etaSeconds),
                    ]
                      .filter((part) => part !== "")
                      .join("  ·  ")}
                  </span>
                  {download.error === null ? null : (
                    <span className="download-error">{download.error}</span>
                  )}
                </span>
              </button>
            );
          })}
        </div>
      )}
    </main>
  );
}
