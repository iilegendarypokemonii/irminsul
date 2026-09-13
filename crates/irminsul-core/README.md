# irminsul-core

Shared Windows capture and UID-scoped account snapshots. The core has no GUI or
optimizer dependency. The native host calls `run_helper_if_requested()` before
normal startup, then owns a `CaptureController` for the application lifetime.

Call `start()`, poll `state()`, choose a completed UID and capture ID, then obtain
that immutable snapshot with `snapshot()`. `Snapshot::export()` selects any
combination of artifacts, characters, weapons, and materials. Stop capture with
`stop_and_wait()` before application update or exit.

The helper uses Windows Packet Monitor, elevates separately, and sends only game
UDP packets to an authenticated loopback connection. The parent holds all
decoding state. No authkey, raw packet log, or inventory file is written by the
core. Hosts decide where to save an explicitly exported snapshot.

The decoder hint is scoped to PID plus process creation time. Each login gets
fresh account, connection, identity, and session-key state. The only routing
identity is UID field 4 of the recognized login response for the pinned protocol;
a cached wish URL is not inventory provenance. Unsupported identity or mappings
withhold the snapshot.

The bundled game-data revision and decoder revision are recorded in
`../../IMPLEMENTATION.md`. Protocol changes require a reviewed update and replay
regressions. The live helper requires Windows 11 24H2 or newer and uses private Packet
Monitor sessions without parsing localized output; other OS backends in upstream Irminsul are not covered by
this fork's multi-account tests.

Private normalized fixtures can be replayed with:

```powershell
cargo run --manifest-path crates/irminsul-core/Cargo.toml --example replay -- output process1:100000001:login-a.pcapng process1:100000002:login-b.pcapng process1:100000001:return-a.pcapng process2:100000001:restart-a.pcapng
```

Never publish raw captures, authkeys, or personal inventories as fixtures.
