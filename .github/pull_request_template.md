<!--
Conventional commit subject for this PR, e.g. feat(storage): add migration 0001.
Every commit must carry a DCO sign-off (`git commit -s`).
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
     the scheduler. A regression beyond 15% fails CI and may not be "fixed" by
     relaxing the budget in this PR (R16). -->

- [ ] This change does not touch a hot path.
- [ ] Measured before and after. Before: `___` After: `___` (% change: `___`)

## Security

<!-- R21-R25 and SECURITY.md. -->

- [ ] No new trust in untrusted input (paths, release names, provider payloads, torrent metadata).
- [ ] No secrets in logs, errors, or telemetry; redaction still covers new fields.
- [ ] No new outbound host, or the new host is documented in the provider registry.
- [ ] No new copyleft code inside the process boundary (R25, ADR 0009).

## Documentation

- [ ] No document is invalidated by this change.
- [ ] The invalidated documents are updated in this PR (R20), or:
- [ ] An ADR is added because a recorded decision changed (R12).

## Checklist

- [ ] Maps to a task ID and satisfies its acceptance criteria.
- [ ] Tests would fail if the logic broke, and cover the applicable §4 cases.
- [ ] No `unsafe`, no `unwrap`/`expect` outside tests, no dead code (R3, R9).
- [ ] No new dependency without a stated reason and a licence check (R2).
- [ ] Every gate in AGENTS §3 passes; nothing was skipped.
- [ ] No scope creep: unrelated findings are filed against their epic (R30).
