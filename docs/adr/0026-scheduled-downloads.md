# ADR 0026 — Scheduled downloads: one table, one in-app ticker

- **Status:** Accepted.
- **Date:** 2026-10-18
- **Evidence:** the schema and the runtime ticker are in this change; the scheduler's five-minute loop and the timing boundary are the open test gap recorded under PG-E13-08.

## Context

An episode or a film that has not been released yet has nothing to acquire. Before this decision the
title page and the episode card offered Download anyway, because the control was gated on
availability alone; the resulting job could only fail after a search. Two product questions follow:
should the control exist at all, and if a person can ask for the download early, where does that
request live and who acts on it?

The alternatives were an OS-level scheduler (launchd, Task Scheduler, systemd timers), a notification
at release time, or an in-app request that the running app acts on. An OS scheduler would keep
working while Peregrine is closed, but it would need three platform integrations, a separate
executable to trigger, and a supported way for that executable to talk to the app or the database.
A notification is not a download, and the request would still need storage.

## Decision

### 1. A request is a row

`download_schedule` stores the account, the title, an optional episode, the first date it may be
fetched (`YYYY-MM-DD`), a state (`scheduled`, `started`, `failed`), and the last error. A generated
`entity_key` gives one row per title and one per episode, because SQLite treats NULLs in a UNIQUE
constraint as distinct and a plain `(title_id, episode_id)` key would allow two whole-title rows.
The table carries a partial index on `(account_id, release_on)` for the due read.

### 2. The app's own ticker, not the OS

A dedicated thread in the desktop shell runs once at startup and every five minutes. It reads due
rows, moves each to `started` **before** starting the acquisition, and calls the same
`start_acquire` the Download button calls. A failure is written on the row; a success leaves the
row as history and the queue owns the job from then on.

The thread runs in-process because the acquisition pipeline, the safety gate, and the torrent engine
are all in-process. An OS scheduler would have to duplicate that path or connect back into it, and
neither is a smaller surface than the tick.

### 3. Cancel deletes the request

Cancelling a `scheduled` row deletes it. A row already `started` is not a schedule any more: the job
is in the transfer queue, and cancelling it is the queue's Cancel. The two acts are kept distinct in
the UI copy so cancelling a request cannot be misread as stopping a transfer.

## Consequences

- The app must be running at the release date, or started later: the startup tick picks up rows whose
  date passed while the app was closed. It does not skip them because they are late.
- The date comparison uses the UTC date of the injected clock, so a schedule fires on the UTC
  boundary rather than local midnight. That is a documented limitation, not a silent one.
- One failed acquisition does not block the rest of the due rows; each row carries its own outcome.
- No retry is attempted after a failure. A person can schedule the same title again, which upserts
  the row back to `scheduled` with the same date.
- The ticker has no test yet: the loop is a thread around a service call, but that call's due-row
  selection and failure recording are untested at the loop level. PG-E13-08 remains open for it.
