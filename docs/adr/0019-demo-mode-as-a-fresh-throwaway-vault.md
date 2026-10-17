# 0019. Demo mode: a fresh throwaway vault, made through the store

- **Status:** Accepted
- **Date:** 2026-10-16

## Context

Phase 15 (PLAN.md §8, §11) needs `papa --demo`: sample data for screenshots, the README's video and anyone trying Papa, without any real family data. Forces at play:

- **Screenshots must never show a real child.** The demo has to be impossible to confuse with, or write into, the father's own vault or settings.
- **The demo should be the real app.** Locking, backup, PDF export and themes should all work in it, because that's what the screenshots show.
- **It must work from an installed package**, with no repository checkout and no extra files beside the binary.
- **Notes need believable dates.** A journal whose notes were all written a second ago doesn't look like one, and the store stamps every new note with "now".
- **`std::env::set_var` is `unsafe` in Rust 2024.** Setting `PAPA_VAULT` and `PAPA_CONFIG_DIR` from `main` would work, but only with an `unsafe` block.

## Decision

- **A fresh vault in a folder of its own, made on every launch:** `papa-demo` in the system's temporary folder (`/tmp/papa-demo` on Linux). `papa_core::demo::create` deletes what the last demo left, then fills a new vault, before the window opens. Settings live in its `config` folder, so changing the theme in the demo doesn't change the father's.
- **The paths follow a `OnceLock`.** `paths::use_demo_dir` sets a process-wide `static OnceLock<PathBuf>` once, in `main`. `vault_path` and `config_dir` check it before the environment variables, so the shares, child and themes folders, which hang off those two, follow as well. It's safe Rust, and it can only be set once.
- **Seeded through `Store`'s public API**: children, labels, notes, image attachments. The one addition is a crate-private `Store::add_note_at`, so the demo can date its notes. Dates are counted back from today in the local time zone, from two days to about three years ago, so the demo always looks current.
- **An invented family:** three children (Maya, Leo and Nora), five labels, sixteen notes with Markdown, and four illustrations generated for the demo (a crayon drawing, a sandcastle, a bike and a birthday cake), with no people in them. The JPEGs (930 KB in all) are compiled in with `include_bytes!`.
- **Visibly a demo:** a "Demo" tag in the title bar and "Papa (demo)" as the window title.
- **Offline only.** Sharing and messages need an account on a relay, so the demo seeds neither. It can still sign in like any vault.
- **A hand-written command line.** `--demo`, `--help` and `--version` are parsed by a small, tested function (`cli.rs`) rather than with `clap`.

Rejected:

- **An in-memory vault** (`Store::open_in_memory`). Backup, the lock and the share replicas all need a file, and the app opens the vault by path. Each would need a special case.
- **A demo vault kept between runs.** The screenshots would depend on whatever someone did in the last demo.
- **A per-process folder** (`papa-demo-<pid>`). Two demos at once couldn't clash, but nothing would ever clean the folders up.

## Consequences

- `papa --demo` is the same binary and the same code paths as a real vault, so every screenshot shows the real app.
- Two demos at once share the folder: the second one wipes the first's vault from under it. It's a demo, so this is accepted.
- The demo's notes are code (`papa-core/src/demo.rs`). A schema change that breaks seeding fails the demo's tests (`seeds_an_invented_family`, `a_new_demo_replaces_the_last`).
- `Store::add_note_at` is `pub(crate)`. Dating notes by hand, for an import say, would make it public.
