# Papa

[![CI](https://github.com/papadavis47/papa/actions/workflows/ci.yml/badge.svg)](https://github.com/papadavis47/papa/actions/workflows/ci.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

A private, local-first journal for fathers. Keep a profile for each of your children, write notes and save photos about them, and, if you choose, share some of those notes with a child, who can read them in the same app, export them to PDF and message you back.

> **Status:** early development, built in public. Phases 0–2 (workspace and CI, app shell, local storage) are done; Phase 3 (notes) is in progress. See [PLAN.md](PLAN.md) for the roadmap.

## Planned features

- Profiles for each child; Markdown and image notes; your own labels; assign a note to one or more children
- Local-first storage with [Turso](https://turso.tech), fully usable offline
- Optional end-to-end encrypted sharing: each child sees **only** the notes shared with them
- Messages between father and child
- PDF export
- Themes, a settings panel (fonts, text size, density) and an optional Vim mode
- Linux and macOS first, Windows later

## Built with

- **Rust** (edition 2024)
- **[GPUI](https://www.gpui.rs)** via **[gpui-kit](https://gpui-kit.com)**, a GPU-accelerated UI framework and component library
- **[Turso](https://turso.tech)**, the Rust rewrite of SQLite, for local storage and sync

## Repository layout

```
crates/
├── papa/        desktop app (the only crate that depends on gpui-kit)
├── papa-core/   domain logic: models, Turso storage, crypto, sharing
├── papa-proto/  API types shared by the app and the relay
├── papa-pdf/    Markdown → PDF export
└── papa-relay/  small server: accounts, invites, scoped tokens
docs/
├── architecture.md
└── adr/         architecture decision records
```

More detail: [docs/architecture.md](docs/architecture.md) and the [decision records](docs/adr/).

## Building

You need a recent stable Rust toolchain ([rustup](https://rustup.rs)).

**Linux** also needs the native libraries GPUI links against (Debian/Ubuntu names):

```sh
sudo apt install build-essential pkg-config libfontconfig-dev libfreetype-dev \
  libwayland-dev libxkbcommon-dev libxkbcommon-x11-dev libx11-xcb-dev libvulkan-dev
```

**macOS** needs the Xcode command line tools (`xcode-select --install`).

Then:

```sh
cargo run                       # run the desktop app
cargo test --workspace          # run all tests
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
```

The app keeps its vault in your OS data directory (`~/.local/share/papa/vault.db` on Linux, `~/Library/Application Support/papa/vault.db` on macOS). To try things out without touching it, point `PAPA_VAULT` at another file:

```sh
PAPA_VAULT=/tmp/papa-dev.db cargo run
```

## Privacy

Real vaults live in your operating system's app-data directory, never in this repository. Anything shared through the cloud is encrypted on your device first, so the server only ever stores ciphertext. Screenshots and test data use invented children and notes.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.
