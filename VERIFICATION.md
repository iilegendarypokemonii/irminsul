# Multi-account verification

Verified on 2026-09-13. This is an initial Windows release; capture requires
Windows 11 24H2 or newer on x64. It does not establish compatibility with future
game protocol changes or an absence of account-enforcement risk.

## Recorded account transitions

Four private login recordings were replayed through one shared Engine. The first
three used one process identity; the final recording used a new process identity.
Private UIDs, inventories, recordings, and logs are excluded from this repository.

| Login | Artifacts | Characters | Weapons | Named material types |
| --- | ---: | ---: | ---: | ---: |
| Account A | 521 | 69 | 202 | 899 |
| Switch to B through title menu | 762 | 79 | 456 | 1160 |
| Return to A through title menu | 521 | 69 | 202 | 900 |
| Restart game and log into A | 521 | 69 | 202 | 900 |

Artifact totals were independently checked in-game when those recordings were
made. Subsequent farming changes inventory; these totals are fixture expectations,
not current account totals. No cross-account artifact GUID overlap was found.
All four latest replays produced no unmapped-material warnings.

## Automated checks

- 9 core tests: session resets, process identity changes, incomplete snapshots,
  conflicting UIDs, stale snapshot IDs, selection, unknown mappings, invalid
  identities, bounded/fragmented IPC frames, clean shutdown acknowledgement, and
  retaining a partially received frame while stopping over a real loopback socket.
- 3 decoder regression tests: missing send time, invalidated session key, and
  preserving invalid item records for the caller to reject.
- Standalone release build and optimizer Rust checks.
- Optimizer integration: 7 import tests, TypeScript and Biome checks, production
  frontend build. Imports preserve absent items and roll back failed swaps/saves.
- Playwright against isolated synthetic browser storage/desktop IPC: home entry,
  account/wishes separation, start/stop controls, UID destination refusal, import
  preview/confirmation, selected export, material viewer, backup download, and
  recovery after a transient status error. No real optimizer database was imported.

## Native check and review

The standalone window was inspected on Windows. The user approved its capture
permission prompt and confirmed capture was running. The first stop exposed
Windows error 10054: two stop bytes were written while the child consumed one.
The parent now writes once and requires a completion acknowledgement after the
helper releases its capture stream. The rebuilt `capture_smoke` check passed:
helper ready, two seconds of capture, acknowledged shutdown, and no stale snapshot.
The elevated helper process exited.

Fable performed an adversarial review and a follow-up review. Its six original
findings were fixed: process-exit race, localized preflight, silent conversion
loss, missing-header decoder stall, sticky errors, and dead settings controls.
The follow-up independently ran core and decoder tests. Its remaining reported
edge cases were addressed: invalidated keys now report failure, the 20-minute
deadline stops cleanly, unavailable process snapshots reset identity, legacy
Windows capture entry points use private sessions, and failed storage swaps are
inside rollback protection. Multiple simultaneous game processes remain rejected
intentionally because capture provenance would otherwise be ambiguous.

A final Fable pass identified partial-frame loss when switching to shutdown;
the stop path now carries its receive buffer into acknowledgement parsing. Its
concern about cache-based rollback led to restoring the exact changed raw entries
instead, with regression coverage for opaque settings and unrelated concurrent
writes. Review conclusions were checked against the code and tests; no unresolved
concrete blocking findings remain from those passes.

The full continuous account-switching workflow in the newly packaged helper has
not yet been repeated live. Recorded transitions prove decoder/session behavior;
the native smoke check proves start/stop and permission plumbing. A live
A -> B -> A sequence remains the next useful acceptance test.

## Complexity report

New capture/session functions remain at or below CCN 10 after extracting login
identity parsing and the transport receive loop.

| Function | Before | After |
| --- | ---: | ---: |
| command | 11 | 9 |
| capture_parent | 18 | 5 |
| genshin_optimizer_ui | 14 | 10 |

Existing
upstream UI `update` (11) and character conversion (13) were not broadly refactored.
Behavior was checked through the tests and replays above.
