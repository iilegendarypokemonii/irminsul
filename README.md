# Irminsul Multi-account

An independent Windows fork of [konkers/irminsul](https://github.com/konkers/irminsul), focused on reliable account switching. It works as a standalone application; no optimizer installation is required. Original project information and credits are preserved below.

## What changed

- Separate, timestamped inventory snapshots for every captured UID.
- Account switches reset inventory, equipment, and connection state while retaining the decoding hint for the current game process. A full restart clears that hint.
- Select artifacts, characters, weapons, materials, or any combination for GOOD export. All rarities and actual artifact levels are preserved.
- Account data and wish history have separate controls. A wish-cache authkey never labels an inventory snapshot.
- Only the packet helper requests administrator permission. Capture errors, cancellation, and incomplete logins are visible; previous completed snapshots retain their account labels.
- A reusable Rust core lets native applications provide the same tool in-app.

## Install and use on Windows

Requires **Windows 11 24H2 or newer, 64-bit Intel/AMD**. The new multi-account interface is currently released for Windows only; the legacy cross-platform backend is not part of this release.

1. Download `irminsul-windows-x64.exe` from [this fork's releases](https://github.com/iilegendarypokemonii/irminsul/releases). If there is no release yet, the build is still being validated; developers can use the source instructions below.
2. Run the executable. No installer, Node.js, or optimizer is required.
3. Start capture and allow the Windows permission prompt. Then log into Genshin and enter the door.
4. Select the captured UID and data categories, then save or copy a GOOD export.
5. Keep capture running when switching accounts through the title menu. Each completed login gets its own snapshot. Stop capture when finished.

Known materials are exported by name. Any materials missing bundled names are identified in a warning and preserved by item ID and quantity in the export metadata.

The snapshot reflects inventory at login, not subsequent farming or upgrades. Begin capture before the first login after launching the game. If the initial login was missed, restart the game with capture running.

Current multi-account support targets the pinned game protocol. Capture uses a private Windows Packet Monitor session without changing global filters or parsing translated command output. Passive capture does not establish that use is permitted by HoYoverse or free from account-enforcement risk. Do not share raw captures or wish URLs.

## Build and verification

Install stable Rust and the Windows C++ build tools, then run `cargo build --release --locked`. The standalone executable is `target/release/irminsul.exe`. Game data is bundled; starting the app does not download it. Updates check this fork's releases.

Run `cargo test --locked --manifest-path crates/irminsul-core/Cargo.toml --lib` for core tests. See [implementation and acceptance criteria](IMPLEMENTATION.md) and [the core interface](crates/irminsul-core/README.md). Source and documentation changes in this fork were assisted by AI and reviewed separately from upstream contributions.

---

## Original Irminsul project

![Screenshot](docs/src/images/main-window.webp)

# Resources

- [Docs](https://konkers.github.io/irminsul)
- [Discord](https://discord.gg/aQqdZPHEpP)

# Introduction

Irminsul is a utility to extract data from Genshin Impact and export it for use with [Genshin Optimizer](https://frzyc.github.io/genshin-optimizer/) and web sites, applications, and utilities that use the [GOOD](https://frzyc.github.io/genshin-optimizer/#/doc) data format.

Irminsul utilizes packet capture instead of the common optical character recognition (OCR) that other [scanners](https://frzyc.github.io/genshin-optimizer/#/scanner) use. This allows it to be much quicker in exchange for 1. needing to run with admin/root privaleges (for the packet capture) and 2. needing to be run when genshin starts to observe the handshake with the server.

## Dependencies

To use the `pcap` capture backend, make sure to install a Pcap library (Npcap/WinPcap on Windows, libpcap on Linux). The released Linux binary has libpcap linked into it, so this only applies there when building Irminsul yourself.

## Command line options

Irminsul accepts a handful of command line options for advanced use cases:

- `--capture-backend <pktmon|pcap>`: chooses which capture backend to use. On Windows both `pktmon` (default) and `pcap` are available. On other platforms only `pcap` is available.
- `--no-admin`: skips the automatic elevation prompt. This can be useful when you prefer to launch the application without requesting higher privileges up front.

## Features

In it's current state Irminsul supports:

- Incredibly fast capture of all Genshin Optimizer supported data
  - Artifacts including "unactivated" rolls and reporting of initial values for rolls
  - Weapons
  - Materials
  - Characters
- Simple, clean UI
- Export settings to filter which data gets exported
- Exports data either to the clipboard or saved to a file

Planned features include:

- Achievement export
- Wish history export
- Real time data updates while game is running

## Thanks

Irmunsil is built upon the work of many others.

- [PJK136](https://github.com/PJK136) whose work on a [fork of `stardb-exporter`](https://github.com/PJK136/stardb-exporter) provided the main inspiration for Irminsul's development.
- [juliuskreutz](https://github.com/juliuskreutz) whose [`stardb-exporter`](https://github.com/juliuskreutz/stardb-exporter) provided the foundation for PJK136's work as well as providing some examples for how to wrangle [`egui`](https://github.com/emilk/egui).
- [hashblen](https://github.com/hashblen) whose [`auto-artifactarioum`](https://github.com/hashblen/auto-artifactarium) is used to interpret the network packets from Genshin.
- [IceDynamix](https://github.com/IceDynamix/) whose work on Honkai Star Rail network scanning is at the root of many of the Genshin and HSR network scanning utilities.
- [emmachase](https://github.com/emmachase) who wrote the packet capture library [`pktmon`](https://github.com/emmachase/pktmon) which Irminsul uses to allow packet capture without having to install a npcap driver as well as their contributions to some of the above projects.
- [Genshin Optimizer](https://frzyc.github.io/genshin-optimizer/) without which there would be no point in exporting data.
- [Inventory Kamera](https://github.com/Andrewthe13th/Inventory_Kamera) which was my introduction into artifact and character scanning and whose discord provided a collaboration environment that spawned Irminsul.
