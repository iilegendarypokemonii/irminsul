# Multi-account Irminsul

This fork provides a standalone Windows application and a reusable Rust core.
The optimizer embeds the same core; Irminsul has no optimizer dependency.

## Acceptance criteria

- Replay the recorded 5 -> 4 -> 5 sequence through one engine and obtain the
  correct captured UID, 521/762/521 artifacts, and no cross-account GUID overlap.
- Observe a changed game process and clear the decoding hint. Replay the restart
  fixture and recover the same 521 artifacts with no earlier hint.
- Missing identity, incomplete snapshots, unknown artifact mappings, malformed
  packets, capture failure, and cancellation never create an exportable snapshot.
  Previous completed snapshots remain explicitly labelled by UID and capture time.
- Select a captured account and any nonempty combination of artifacts, characters,
  weapons, and materials. Exports contain only selected categories, real levels,
  and the correct GOOD `elixirCrafted` field.
- Keep wish-cache authkeys separate from captured inventory identity. Show wishes
  and account data separately in both interfaces.
- Elevate only a short-lived packet helper. It connects to its parent over an
  authenticated loopback connection, captures only Genshin UDP ports, and exits on
  stop, parent disconnection, or timeout. Raw packets are not saved by default.
- Optimizer integration exposes the complete capture/export workflow in Tools and
  on the home game-data card. Imports require an exact, unique destination UID,
  a preview, and explicit confirmation. Preserve items absent from the snapshot.
- Verify core behavior with synthetic tests and private capture replay; verify the
  optimizer UI with Playwright. Obtain an independent adversarial code review.

## Source and data

Upstream Irminsul: 781006e82d76b29b10b21125aa3bc1b79ddf7b3c.
Vendored MIT decoder: konkers/auto-artifactarium at
4ba25fac64b88970143af6bc2a2ef51338e620d0, with local session and parser fixes.
Its KCP dependency is pinned to 1acf4ba5938ff91f7f2d2a31e16bf1f8d2db9c8f.
Vendored pktmon 0.6.2 retains its MIT license and exposes a private-session entry
point. Capture requires Windows 11 24H2 or newer; no translated CLI output or
global filter mutation is used. See [verification results](VERIFICATION.md).
Bundled public game data: 26df1dfbdf05a82bbb1d97506859f3e1c40718d8.
Private recordings stay outside this repository.
