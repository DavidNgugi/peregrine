import { lazy, Suspense, useCallback, useEffect, useRef, useState, type ReactElement } from "react";
import { createPortal } from "react-dom";

import { api, type Profile, type Tile } from "./api.js";
import { avatarSrc, clearLegacyProfiles, hueFor, initialsFor, legacyProfiles } from "./profiles.js";

const ProfileEditor = lazy(() =>
  import("./ProfileEditor.js").then((module) => ({ default: module.ProfileEditor })),
);

/** The avatar: the picture the profile stores, else its initials over its hue. */
function Avatar({
  profile,
  size = "normal",
}: {
  readonly profile: Profile | null;
  readonly size?: "normal" | "large";
}): ReactElement {
  const src = profile === null ? null : avatarSrc(profile.avatarRef, profile.name);
  const name = profile?.name ?? "";
  return (
    <span
      aria-hidden="true"
      className={size === "large" ? "profile-avatar profile-avatar-large" : "profile-avatar"}
      style={{ background: `hsl(${String(hueFor(name))} 52% 42%)` }}
    >
      {src === null ? initialsFor(name) : <img alt="" src={src} />}
    </span>
  );
}

/**
 * The household's profile picker.
 *
 * The list, the active choice, and the watch state scoped by it all live in the core, so a
 * profile added on the desktop appears here on the next read and selecting one reloads this
 * client under that account. A kids profile is the restricted scope: adult and unrated titles
 * disappear from every read, not just from this grid.
 */
export function ProfileMenu(): ReactElement {
  const [profiles, setProfiles] = useState<Profile[]>([]);
  const [active, setActive] = useState<Profile | null>(null);
  const [open, setOpen] = useState(() => sessionStorage.getItem("profile-selection") === "open");
  const [adding, setAdding] = useState(false);
  const [editing, setEditing] = useState(false);
  const [backdrops, setBackdrops] = useState<string[]>([]);
  const [artIndex, setArtIndex] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const dialog = useRef<HTMLDivElement | null>(null);
  const activeId = useRef<number | null>(null);

  const load = useCallback(async (): Promise<void> => {
    try {
      const rows = await api.profiles();
      if (rows.length === 1 && rows[0]?.name === "You") {
        const legacy = legacyProfiles().filter((profile) => profile.name !== "You");
        if (legacy.length > 0) {
          let chosen = rows[0].id;
          for (const old of legacy) {
            const created = await api.createProfile(old.name, "adult");
            if (old.active) chosen = created.id;
          }
          await api.selectProfile(chosen);
          clearLegacyProfiles();
          window.location.reload();
          return;
        }
        clearLegacyProfiles();
      }
      const next = rows.find((profile) => profile.active) ?? rows[0] ?? null;
      if (activeId.current !== null && next !== null && next.id !== activeId.current) {
        // Another client switched the installation; this page's data is stale under the old
        // scope, so take the new one from the top rather than redraw half an answer.
        window.location.reload();
        return;
      }
      activeId.current = next?.id ?? null;
      setProfiles(rows);
      setActive(next);
    } catch {
      // The shell's pairing gate owns the failure; the menu keeps what it last drew.
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  useEffect(() => {
    if (open) void load();
  }, [load, open]);

  useEffect(() => {
    const timer = window.setInterval(() => {
      void load();
    }, 10_000);
    return () => {
      window.clearInterval(timer);
    };
  }, [load]);

  useEffect(() => {
    if (!open) return;
    const previous = document.activeElement;
    const root = document.getElementById("root");
    root?.setAttribute("inert", "");
    const previousOverflow = document.body.style.overflow;
    document.body.style.overflow = "hidden";
    dialog.current?.querySelector<HTMLElement>("button, input")?.focus();
    const onKey = (event: KeyboardEvent): void => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      if (adding) setAdding(false);
      if (editing) setEditing(false);
    };
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("keydown", onKey);
      root?.removeAttribute("inert");
      document.body.style.overflow = previousOverflow;
      if (previous instanceof HTMLElement && previous.isConnected) previous.focus();
    };
  }, [open, adding, editing, profiles.length]);

  useEffect(() => {
    if (open) sessionStorage.setItem("profile-selection", "open");
  }, [open]);

  useEffect(() => {
    if (
      !open ||
      backdrops.length < 2 ||
      window.matchMedia("(prefers-reduced-motion: reduce)").matches
    )
      return;
    const timer = window.setInterval(
      () => setArtIndex((index) => (index + 1) % backdrops.length),
      7000,
    );
    return () => window.clearInterval(timer);
  }, [open, backdrops.length]);

  useEffect(() => {
    if (!open) setAdding(false);
  }, [open]);

  useEffect(() => {
    if (!open) return;
    void api
      .home()
      .then((home) => {
        const candidates = [home.hero, ...home.heroes, ...home.rails.flatMap((rail) => rail.items)];
        const tiles = candidates.filter((tile): tile is Tile => tile !== null);
        const wide = tiles.flatMap((tile) => (tile.backdrop === null ? [] : [tile.backdrop]));
        const posters = tiles.flatMap((tile) => (tile.poster === null ? [] : [tile.poster]));
        setBackdrops([...new Set(wide.length > 0 ? wide : posters)].slice(0, 8));
        setArtIndex(0);
      })
      .catch(() => {
        setBackdrops([]);
      });
  }, [open]);

  const choose = (profile: Profile): void => {
    void api
      .selectProfile(profile.id)
      .then(() => {
        sessionStorage.removeItem("profile-selection");
        window.location.reload();
      })
      .catch((caught: unknown) => {
        setError(caught instanceof Error ? caught.message : "The profile could not be changed.");
      });
  };

  return (
    <>
      <button
        aria-expanded={open}
        aria-haspopup="dialog"
        aria-label={"Switch profile. Now " + (active?.name ?? "nobody")}
        className="profile-button"
        data-focus="true"
        onClick={() => setOpen((held) => !held)}
        type="button"
      >
        <Avatar profile={active} />
      </button>
      {open
        ? createPortal(
            <div className="profile-overlay profile-full-page-overlay">
              {backdrops.map((src, index) => (
                <img
                  alt=""
                  className={index === artIndex ? "profile-backdrop visible" : "profile-backdrop"}
                  key={src}
                  src={src}
                />
              ))}
              <div aria-hidden="true" className="profile-backdrop-scrim" />
              <div
                aria-label="Who is watching"
                aria-modal="true"
                className="profile-dialog profile-full-page"
                data-focus-scope="profile"
                ref={dialog}
                role="dialog"
                onKeyDown={(event) => {
                  if (event.key !== "Tab") return;
                  const controls = Array.from(
                    dialog.current?.querySelectorAll<HTMLElement>(
                      "button:not(:disabled), input:not(:disabled)",
                    ) ?? [],
                  );
                  const first = controls[0];
                  const last = controls[controls.length - 1];
                  if (event.shiftKey && document.activeElement === first) {
                    event.preventDefault();
                    last?.focus();
                  } else if (!event.shiftKey && document.activeElement === last) {
                    event.preventDefault();
                    first?.focus();
                  }
                }}
              >
                {adding || editing ? (
                  <Suspense fallback={<p role="status">Loading profile editor…</p>}>
                    <ProfileEditor
                      initialName={editing ? (active?.name ?? "") : ""}
                      initialAvatar={editing ? (active?.avatarRef ?? null) : null}
                      editing={editing}
                      onCancel={() => {
                        setAdding(false);
                        setEditing(false);
                      }}
                      onSave={async (name, avatar, kids) => {
                        if (editing) await api.editProfile(name, avatar);
                        else await api.createProfile(name, kids ? "child" : "adult", avatar);
                        await load();
                        setAdding(false);
                        setEditing(false);
                      }}
                    />
                  </Suspense>
                ) : (
                  <>
                    <div className="profile-dialog-head">
                      <h2>Who's watching?</h2>
                    </div>
                    <div className="profile-list">
                      {profiles.map((profile) => (
                        <div className="profile-selection-row" key={profile.id}>
                          <button
                            aria-label={profile.name}
                            aria-pressed={profile.active}
                            className={profile.active ? "profile-choice current" : "profile-choice"}
                            data-focus="true"
                            data-profile-avatar="true"
                            onClick={() => choose(profile)}
                            type="button"
                          >
                            <Avatar profile={profile} size="large" />
                          </button>
                          <span>{profile.name}</span>
                          {profile.restricted ? <small>Kids</small> : null}
                          {profile.active && (
                            <button
                              className="profile-edit-trigger"
                              data-focus="true"
                              aria-label={`Edit ${profile.name}`}
                              type="button"
                              onClick={() => setEditing(true)}
                            >
                              <svg
                                aria-hidden="true"
                                viewBox="0 0 24 24"
                                width="22"
                                height="22"
                                fill="none"
                                stroke="currentColor"
                                strokeWidth="1.5"
                              >
                                <path d="m4 16 12-12 4 4L8 20H4v-4Zm9-9 4 4" />
                              </svg>
                            </button>
                          )}
                        </div>
                      ))}
                    </div>
                    <button
                      aria-label="Add a profile"
                      className="profile-add-trigger"
                      data-focus="true"
                      onClick={() => setAdding(true)}
                      type="button"
                    >
                      <span aria-hidden="true" className="profile-add-plus">
                        +
                      </span>
                      <span>Shared on this installation</span>
                    </button>
                  </>
                )}
                {error === null ? null : <p className="profile-error">{error}</p>}
              </div>
            </div>,
            document.body,
          )
        : null}
    </>
  );
}
