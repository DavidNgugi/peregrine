import { useCallback, useEffect, useId, useRef, useState, type ReactElement } from "react";

import { api, type Profile } from "./api.js";
import { avatarSrc, clearLegacyProfiles, hueFor, initialsFor, legacyProfiles } from "./profiles.js";

/** The avatar: the picture the profile stores, else its initials over its hue. */
function Avatar({
  profile,
  size = "normal",
}: {
  readonly profile: Profile | null;
  readonly size?: "normal" | "large";
}): ReactElement {
  const src = profile === null ? null : avatarSrc(profile.avatarRef);
  const name = profile?.name ?? "";
  return (
    <span
      aria-hidden="true"
      className={size === "large" ? "profile-avatar profile-avatar-large" : "profile-avatar"}
      style={src === null ? { background: `hsl(${String(hueFor(name))} 52% 40%)` } : undefined}
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
  const [open, setOpen] = useState(false);
  const [name, setName] = useState("");
  const [kids, setKids] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const dialog = useRef<HTMLDivElement | null>(null);
  const activeId = useRef<number | null>(null);
  const addInputId = useId();

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
    dialog.current?.querySelector<HTMLElement>("button, input")?.focus();
    const onKey = (event: KeyboardEvent): void => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      setOpen(false);
    };
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("keydown", onKey);
      if (previous instanceof HTMLElement && previous.isConnected) previous.focus();
    };
  }, [open]);

  const choose = (profile: Profile): void => {
    void api
      .selectProfile(profile.id)
      .then(() => {
        window.location.reload();
      })
      .catch((caught: unknown) => {
        setError(caught instanceof Error ? caught.message : "The profile could not be changed.");
      });
  };

  const add = (): void => {
    const trimmed = name.trim();
    if (trimmed.length === 0) return;
    void api
      .createProfile(trimmed, kids ? "child" : "adult")
      .then(() => {
        window.location.reload();
      })
      .catch((caught: unknown) => {
        setError(caught instanceof Error ? caught.message : "The profile could not be created.");
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
        <span className="profile-button-name">{active?.name ?? "Profile"}</span>
      </button>
      {open ? (
        <div
          className="profile-overlay"
          onClick={(event) => {
            if (event.target === event.currentTarget) setOpen(false);
          }}
        >
          <div
            aria-label="Who is watching"
            aria-modal="true"
            className="profile-dialog"
            ref={dialog}
            role="dialog"
          >
            <div className="profile-dialog-head">
              <h2>Who's watching?</h2>
              <button
                className="profile-close"
                data-focus="true"
                onClick={() => setOpen(false)}
                type="button"
              >
                Close
              </button>
            </div>
            <div className="profile-list">
              {profiles.map((profile) => (
                <button
                  aria-pressed={profile.active}
                  className={profile.active ? "profile-choice current" : "profile-choice"}
                  data-focus="true"
                  key={profile.id}
                  onClick={() => choose(profile)}
                  type="button"
                >
                  <Avatar profile={profile} size="large" />
                  <span>{profile.name}</span>
                  {profile.restricted ? <small>Kids</small> : null}
                </button>
              ))}
            </div>
            <form
              className="profile-add"
              onSubmit={(event) => {
                event.preventDefault();
                add();
              }}
            >
              <label htmlFor={addInputId}>Add a profile</label>
              <div className="profile-add-row">
                <input
                  autoComplete="off"
                  data-focus="true"
                  id={addInputId}
                  maxLength={40}
                  onChange={(event) => setName(event.target.value)}
                  placeholder="Name"
                  value={name}
                />
                <button data-focus="true" disabled={name.trim().length === 0} type="submit">
                  Add
                </button>
              </div>
              <label className="profile-kids">
                <input
                  checked={kids}
                  onChange={(event) => setKids(event.target.checked)}
                  type="checkbox"
                />
                Kids profile, which hides adult and unrated titles
              </label>
            </form>
            {error === null ? null : <p className="profile-error">{error}</p>}
            <p className="profile-note">
              Profiles are shared by this installation. The watchlist, progress, and history belong
              to the profile you choose.
            </p>
          </div>
        </div>
      ) : null}
    </>
  );
}
