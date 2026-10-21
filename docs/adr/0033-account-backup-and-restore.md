# ADR 0033 — Account backup and restore

- **Status:** Accepted.
- **Date:** 2026-10-21
- **Related:** [0003](./0003-sqlite-and-fts5.md), [0019](./0019-local-accounts-and-parental-gating.md), [0032](./0032-shared-profiles.md)

## Context

The household's data is spread across three places by design: profiles, wishlists, watch
progress, history, settings, the catalogue, the library index, and schedules live in the
SQLite database; credentials live in the OS keychain, referenced by `secret_ref` rows;
and the films and series themselves are files in library folders. Before this record the
only copies the app took were the pre-migration rollback copies beside the database, which
are rotated to three and are not a user-facing backup: they disappear, they are not
portable, and they are taken only when the schema changes.

A person asking to "back up my account" means the household state, not the media: they want
their profiles, their lists, their progress, their settings, and their schedules back after a
reinstall or on another machine. They explicitly do not expect a copy of every downloaded
file, and copying one is not a reasonable thing for a settings action to attempt.

## Decision

1. **A backup is one SQLite file produced by `VACUUM INTO`.** That is the same mechanism as
   the pre-migration copy, and it is the only way to get a consistent snapshot out of a WAL
   database through an open connection. The file is written under a `.partial` name and
   renamed once complete, so a crash cannot leave a half-written file that reads like a backup.
2. **What is in the backup.** The whole database, with the rows that are about *this process*
   or *this machine* removed: the scheduler journal, the acquisition queue and its decision
   trail, the provider and image caches, casting and paired-device sessions, the web audit
   log, and the diagnostics event log. Profiles, wishlists, watch progress, history,
   settings, the catalogue, the library index, match rules, schedules, notifications, and
   keyring `secret_ref` rows remain.
3. **What is not in the backup, and is said in the UI.** Downloaded media files; cached
   artwork blobs; and the credential values themselves, which were never in the database.
   `secret_ref` rows name keyring entries, so a restore on the same machine keeps working,
   and a restore on another machine asks for its keys again. Library paths are stored as
   they were, so a restore on a machine with a different folder layout shows the same
   catalogue and watch state with the media marked missing until the folders are re-added.
4. **A restore is staged, then applied by the next launch.** The live database has a writer
   task and a reader pool; SQLite offers no supported swap beneath them. The chosen file is
   inspected, copied to `restore/pending.sqlite` under the data directory, and the app is
   restarted. On the next start, before any connection is opened, the current database is
   copied to `restore/previous.sqlite` and the staged file is renamed into place.
5. **Inspection is the boundary.** A candidate is opened read-only, checked with
   `PRAGMA integrity_check`, required to hold the Peregrine tables, and refused when its
   newest migration is above this build's. An older backup is accepted and upgraded by the
   ordinary migration runner on the next launch.
6. **A failed restore puts the previous database back.** If the restored file cannot be
   opened, the runtime moves it to `restore/failed.sqlite`, restores the previous database,
   and starts on it. The failed file is kept for diagnosis, not deleted.
7. **The action lives in Settings → Data.** The user chooses the destination folder for a
   backup and the source file for a restore in the OS dialog; the core canonicalizes both,
   generates the backup's file name, and never treats a picked path as trusted input.

## Alternatives considered

| Alternative | Why not |
| --- | --- |
| Copy the database file with the filesystem | A WAL database's file alone is not a consistent snapshot; recent commits may still be in the `-wal` file. |
| Export every user table to JSON | Re-importing has to reconstruct foreign keys, triggers, and FTS rows, and every schema change becomes a second migration path. The database is already the document. |
| Replace the database under the running process | The writer owns the only write connection and the reader pool holds open snapshots. Closing and rebuilding the runtime would invalidate every service that holds an `Arc`. |
| Include media files in the archive | A backup would be hundreds of gigabytes, slow enough that nobody takes one, and it duplicates files the library already owns. |
| Copy keyring values into the backup | A backup file would become a credential store outside the OS keychain, which is exactly what SECURITY.md forbids. |
| Keep the acquisition queue | A download without its partial bytes cannot resume, and a restored "downloading" row claims work that no engine is doing. |

## Consequences

- A restore takes two launches, and the UI says so. A user who dismisses the restart still has
  a staged file: the next normal launch applies it.
- The download queue and its history are not part of a backup. Schedules are.
- Credentials must be re-entered on a different machine; the backup is not a migration of the
  keychain.
- The catalogue is included, so wishlists and history render without a provider on first
  launch. Artwork is not, so posters return on the next provider refresh.
- The backup is a `*.sqlite` file rather than a bespoke container. It can be inspected with
  any SQLite tool, which is a feature for a user who wants to verify what they handed over.
