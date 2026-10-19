# Usenet provider spike (SPIKE-20)

**Status: partially executed.** No provider account was available in this environment, so the
throughput, authentication, and missing-article halves of SPIKE-20 are unmeasured. What follows
records exactly what was and was not checked, so a later run does not mistake the empty half for a
pass.

## What was verified

- **TLS and greeting, one real provider.** On 2026-10-19, a TLS connection to
  `news.eternal-september.org:563` with the public trust store returned
  `200 nntp.eternal-september.org InterNetNews NNRP server INN 2.8.0 ... ready`. This is the same
  path `NntpConnection::connect` takes: TCP, implicit TLS, certificate check, greeting.
- **Local end-to-end transfer.** A loopback NNTP fixture serves a two-part yEnc post; the engine
  writes `helloworld` at the offsets the part headers name. Missing articles, pause/resume, a
  two-file/two-connection run, and remove/cancel are covered by tests.
- **Repair and extraction with the real tools.** The pinned 7zz listed and extracted a real ZIP;
  the pinned par2 verified a real par2 set. The app's listing parser was exercised against 7zz's
  real `-slt` output.

## What was not verified

- **Authentication.** No account, so AUTHINFO USER/PASS against a provider is unmeasured. The
  loopback fixture covers the exchange.
- **Eight-connection throughput.** No account, no billed connections, no sustained transfer.
- **Missing-article rate on a real retention window.**
- **A scene RAR set from a real provider.** The extract path is shared with the verified ZIP; a
  RAR fixture is not stored in the tree.

## What a complete run needs

Two provider accounts (one primary, one backup), the same host and network the release will claim,
and a 10–20 GB scene set with a par2 set. Record: TLS/auth result, per-connection throughput at 1,
2, 4, and 8 connections, missing-article count, par2 repair result, extraction result, and the wall
time for repair → extract → import. Do not record a number from a different network or host.
