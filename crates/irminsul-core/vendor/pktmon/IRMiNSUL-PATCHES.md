# Local changes to pktmon 0.6.2 (MIT)

Copied from the published crates.io source. Original license and attribution are
preserved. `Capture::isolated()` exposes only the private real-time backend and
never falls back to the legacy backend. Irminsul uses this entry point because
the legacy constructor stops existing captures and deletes global filters.

This requires the Packet Monitor real-time API (Windows 11 24H2 or newer).
No localized command output is parsed. The public `Capture::new()` API is retained
for upstream compatibility but is not used by the shared Irminsul capture core.

Native callbacks use the Windows `system` ABI (including x64). The real-time
API library is loaded from System32 only.
