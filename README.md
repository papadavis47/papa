# Papa

[![CI](https://github.com/papadavis47/papa/actions/workflows/ci.yml/badge.svg)](https://github.com/papadavis47/papa/actions/workflows/ci.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

A private, local-first journal for fathers. Keep a profile for each of your children, write notes and save photos about them, and, if you choose, share some of those notes with a child, who can read them in the same app, export them to PDF and message you back.

> **Status:** early development, built in public. Phases 0–13 (workspace and CI, app shell, local storage, notes, images, settings and themes, command palette, Vim mode, PDF export, vault lock and backup, the relay server, sharing notes from the father's side, child mode for reading them, and messages between father and child) are done. See [PLAN.md](PLAN.md) for the roadmap.

## Planned features

- Profiles for each child; Markdown and image notes; your own labels; assign a note to one or more children
- Local-first storage with [Turso](https://turso.tech), fully usable offline
- Optional end-to-end encrypted sharing: each child sees **only** the notes shared with them
- Messages between father and child
- PDF export: one note, the notes a label or search picks out, or a child's whole book with a title page and contents, typeset with [Typst](https://typst.app)
- Themes, a settings panel (fonts, text size, density) and an optional Vim mode (Normal, Insert and Visual modes, counts and operators, `:w`)
- Keyboard first: a ⌘K command palette reaches every command, child, label and note
- Linux and macOS first, Windows later

## Built with

- **Rust** (edition 2024)
- **[GPUI](https://www.gpui.rs)** via **[gpui-kit](https://gpui-kit.com)**, a GPU-accelerated UI framework and component library
- **[Turso](https://turso.tech)**, the Rust rewrite of SQLite, for local storage and sync
- **[Typst](https://typst.app)**, run in-process as a library, for PDF export

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

Settings live apart from the vault, in `~/.config/papa/settings.json` on Linux and `~/Library/Application Support/papa/settings.json` on macOS, with your own theme files in a `themes` folder next to it. `PAPA_CONFIG_DIR` points both somewhere else:

```sh
PAPA_VAULT=/tmp/papa-dev.db PAPA_CONFIG_DIR=/tmp/papa-dev-config cargo run
```

### The relay server

`papa-relay` handles accounts, invites and access to shared notes. For development it runs against Turso's local sync server, so no cloud account is needed. You need the `tursodb` CLI ([Turso releases](https://github.com/tursodatabase/turso/releases), v0.8.2):

```sh
mkdir -p dev-sync   # tursodb won't create it
tursodb --sync-server 127.0.0.1:8080 --sync-dir dev-sync
PAPA_RELAY_LOCAL_SYNC_DIR=dev-sync cargo run -p papa-relay
cargo run -p papa-relay -- admin invite create "Me"   # in another terminal
```

Its settings are environment variables, listed in [.env.example](.env.example).

### Sharing notes (father's side)

With `tursodb` and the relay running as above, start the app, then:

1. **Sign in.** ⌘K → "Sign in to Papa…", or Settings → Sharing. The server is `http://127.0.0.1:8787` by default; "Create account…" takes the invite code from `admin invite create`.
2. **Turn sharing on for a child.** Right-click them in the sidebar → Sharing… → Start sharing (or ⌘K → "Sharing with Emma…"). The dialog shows the child's access key, `PAPA-XXXX-…`, to copy, replace or revoke.
3. **Share a note.** Open a note about that child, then tick "Emma can read it" under the + next to its date (or ⌘K → "Emma can read it").

Papa encrypts the shared notes on your computer, writes them to `shares/<child>.db` next to the vault, and pushes them a few seconds after each change. The status bar shows the last sync.

### Reading shared notes (child's side)

A child uses the same app. On a computer with no vault, Papa asks what you'd like to do; to try it next to your own vault, give it another folder:

```sh
PAPA_VAULT=/tmp/papa-child/vault.db cargo run
```

Choose "Open notes shared with me", type Dad's email and paste the key from his sharing dialog. Papa downloads the share to `child/share.db` next to where the vault would be, decrypts it, and shows the notes read-only. ⌘R checks for new ones (it also does every 30 seconds), ⌘P and ⌘⇧P export to PDF, and the notes stay readable offline. If Dad makes a new key, the app says the old one stopped working and asks for the new one.

### Messages

Dad and a child he shares with can write to each other. In Dad's app, Messages is at the foot of the sidebar (⌘⇧M, or ⌘K → "Write to Emma"); in the child's, it's next to Notes from Dad. Both apps check for new messages every 30 seconds and send at once. A message written offline says "Sending…" and goes with the next check. Messages are encrypted like notes, and Dad's vault keeps the conversation even if sharing stops.

Some tests start `tursodb` to sync for real. They look for it on the `PATH` (or in `$TURSODB`) and skip when it's missing; CI installs it and sets `PAPA_REQUIRE_TURSODB=1`, so there they can't skip.

## Privacy

Real vaults live in your operating system's app-data directory, never in this repository. A vault can be locked with a password, which encrypts it on disk (Settings → Privacy); without the password the file is unreadable, and a forgotten password can't be recovered. Settings → Data → Back up saves the whole vault as one file to keep elsewhere. Anything shared through the cloud is encrypted on your device first, so the server only ever stores ciphertext: never a note's text, its labels, or which children it's about. Screenshots and test data use invented children and notes.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.
