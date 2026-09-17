# irminsul-core

Shared Windows capture and UID-scoped account snapshots. The core has no GUI or
optimizer dependency. The native host calls `run_helper_if_requested()` before
normal startup, then owns a `CaptureController` for the application lifetime.

Call `start()`, poll `state()`, choose a completed UID and capture ID, then obtain
that immutable snapshot with `snapshot()`. `Snapshot::export()` selects any
combination of artifacts, characters, weapons, and materials. Stop capture with
`stop_and_wait()` before application update or exit.

The helper chooses private Windows Packet Monitor or Windows 10-compatible
Winsock capture, elevates separately, and sends only game
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
regressions. `start()` selects the available backend automatically;
`start_with_mode(CaptureMode::Compatibility)` explicitly chooses compatibility
capture. The helper argument accepts only `auto`, `packetMonitor`, or `compatibility`.

The compatibility backend uses Windows' documented `SIO_RCVALL` with
`RCVALL_IPLEVEL` on active IPv4 addresses. It does not enable promiscuous mode,
install a driver, or touch global Packet Monitor state. It filters both directions
of UDP ports 22101/22102 before sending packets to the parent, wraps IPv4 packets
for the existing Ethernet decoder, and bounds its queue to 4 MiB / 4096 packets.
Receive sockets close on stop, parent disconnect, error, or the four-hour deadline.
Changing network/VPN configuration requires restarting capture. IPv6-only capture
is not provided by the compatibility backend.

Reference: [Microsoft SIO_RCVALL documentation](https://learn.microsoft.com/en-us/windows/win32/winsock/sio-rcvall).
The legacy Packet Monitor backend that resets global state remains unused.

Private normalized fixtures can be replayed with:

```powershell
cargo run --manifest-path crates/irminsul-core/Cargo.toml --example replay -- output process1:100000001:login-a.pcapng process1:100000002:login-b.pcapng process1:100000001:return-a.pcapng process2:100000001:restart-a.pcapng
```

Never publish raw captures, authkeys, or personal inventories as fixtures.

Since 0.4.1, `start_with_mode(CaptureMode::PacketMonitor)` explicitly selects the private Windows 11 24H2+ backend and reports failure instead of falling back. `Auto` still falls back to Winsock. `CaptureState.active_backend` reports the helper-confirmed method while capture is running, survives account changes, and clears when capture stops.

For a live comparison on Windows 11 24H2+, run the `compare_capture` example with a private output directory. It starts both methods, writes `status.json` when both helpers are ready, and compares complete inventories from the next login, including artifact GUIDs. Outputs contain private account data; do not commit them. Both capture helpers are stopped before results are compared.
