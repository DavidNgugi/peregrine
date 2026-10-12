<!--
Read AGENTS.md before filling this in: it is the contract this template audits.
Conventional subject (`feat(scope):`, `docs:`, …) and a DCO sign-off on every
commit (R27). Keep the PR to one task (R28). Delete a section that genuinely
does not apply rather than answering it with "n/a".
-->

## Task

<!-- A task ID from docs/TASKS.md. If this work is not in the backlog, the first
     commit adds it. R26: every change maps to a task ID. -->

Task: PG-E0-XX Epic: E0

## What changed

<!-- One paragraph. What is different after this PR, in domain terms. -->

## Why this approach

<!-- The alternatives you rejected and the concrete reason. If this reverses or
     bends a recorded decision, link the ADR you added or superseding. R12. -->

## Evidence

<!-- The proof, not the intent. Command output, recorded numbers, screenshots,
     or a link to the CI run. "Tests pass" is not evidence; paste the summary. -->

```
paste the relevant gate output here
```

## Edge cases walked (AGENTS §4)

<!-- Tick what applies and name the test that covers it. Unticked categories
     need a one-line reason, e.g. "no filesystem access in this change". -->

- [ ] Empty
- [ ] Single
- [ ] Boundary
- [ ] Threshold
- [ ] Malformed
- [ ] Hostile
- [ ] Duplicate
- [ ] Concurrent
- [ ] Interrupted
- [ ] Missing
- [ ] Full
- [ ] Slow
- [ ] Permissions
- [ ] Platform
- [ ] Locale
- [ ] Time
- [ ] Reversal

## Performance

<!-- Required if this touches startup, grid rendering, search, IPC, storage, or
     the scheduler. State the budget it is measured against from AGENTS §6, the
     number before and after, and the machine and method the numbers came from.
     An enforced CI budget gate does not exist yet (PG-E27-08); this section is
     what a reviewer reads until it does. -->

- [ ] This change does not touch a hot path.
- [ ] Measured against the §6 budget. Before: `___` After: `___` (% change: `___`), on `___`.

## Security

<!-- The per-PR controls from SECURITY.md. The release-wide gates stay in its
     "Release security checklist" and are not repeated here. -->

- [ ] No new trust in untrusted input: paths go through `PathGuard`, provider strings render as text, nothing external is executed or opened with the system handler.
- [ ] Downloaded, torrent, or provider-supplied data cannot reach the filesystem, a subprocess argument, or the DOM as a new field (T1–T9).
- [ ] No new outbound host, or the new host is documented in the provider registry with conditional requests and a rate limit (R24).
- [ ] No secret in a log, error, diagnostics bundle, or crash payload, and redaction covers any field this PR adds (T13).
- [ ] No new copyleft code inside the process boundary, and no new dependency without a stated reason and a licence check against `deny.toml` (R2, R25).
- [ ] Crash reporting, telemetry, and any LAN or agent listener stay opt-in, off by default, and unchanged by this PR (R23, R33).

## Documentation

- [ ] No document is invalidated by this change.
- [ ] The invalidated documents are updated in this PR (R20), or:
- [ ] An ADR is added because a recorded decision changed (R12).

## Checklist

- [ ] Maps to a task ID and satisfies its acceptance criteria.
- [ ] Tests would fail if the logic broke, and cover the applicable §4 cases.
- [ ] No `unsafe`, no `unwrap`/`expect` outside tests, no dead code (R3, R9).
- [ ] Every gate in AGENTS §3 that applies to this change ran and passed here; nothing was skipped (R29).
- [ ] Every commit is a conventional commit with a DCO sign-off (`git commit -s`), and the PR title follows the same convention (R27).
- [ ] No scope creep: unrelated findings are filed against their epic (R30).
