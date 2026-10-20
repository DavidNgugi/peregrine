import { useEffect, useId, useRef, useState, type ReactElement } from "react";

import {
  activeProfile,
  initialsFor,
  loadProfileBook,
  saveProfileBook,
  selecting,
  withProfile,
  type ProfileBook,
  type WebProfile,
} from "./profiles.js";

/** The avatar circle: initials over the profile's own hue. */
function Avatar({
  profile,
  size = "normal",
}: {
  readonly profile: WebProfile;
  readonly size?: "normal" | "large";
}): ReactElement {
  return (
    <span
      aria-hidden="true"
      className={size === "large" ? "profile-avatar profile-avatar-large" : "profile-avatar"}
      style={{ background: `hsl(${String(profile.hue)} 52% 40%)` }}
    >
      {initialsFor(profile.name)}
    </span>
  );
}

/**
 * The browser's profile selector.
 *
 * The desktop has the same device-local list in its sidebar. On the web the list lives in this
 * browser's storage and is a label rather than an account: it does not scope the catalogue,
 * progress, or the watchlist, and the note in the dialog says so. A shared profile is ADR 0019's
 * account work, not this menu.
 */
export function ProfileMenu(): ReactElement {
  const [book, setBook] = useState<ProfileBook>(loadProfileBook);
  const [open, setOpen] = useState(false);
  const [name, setName] = useState("");
  const dialog = useRef<HTMLDivElement | null>(null);
  const addInputId = useId();
  const active = activeProfile(book);

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

  const choose = (id: string): void => {
    const next = selecting(book, id);
    setBook(next);
    saveProfileBook(next);
    setOpen(false);
  };

  const add = (): void => {
    const next = withProfile(book, name);
    if (next === book) return;
    setBook(next);
    saveProfileBook(next);
    setName("");
    setOpen(false);
  };

  return (
    <>
      <button
        aria-expanded={open}
        aria-haspopup="dialog"
        aria-label={"Switch profile. Now " + active.name}
        className="profile-button"
        data-focus="true"
        onClick={() => setOpen((held) => !held)}
        type="button"
      >
        <Avatar profile={active} />
        <span className="profile-button-name">{active.name}</span>
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
              {book.profiles.map((profile) => (
                <button
                  aria-pressed={profile.id === active.id}
                  className={profile.id === active.id ? "profile-choice current" : "profile-choice"}
                  data-focus="true"
                  key={profile.id}
                  onClick={() => choose(profile.id)}
                  type="button"
                >
                  <Avatar profile={profile} size="large" />
                  <span>{profile.name}</span>
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
            </form>
            <p className="profile-note">
              Profiles are local to this browser. They do not change the library or watch state.
            </p>
          </div>
        </div>
      ) : null}
    </>
  );
}
