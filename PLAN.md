# Papa — Plan

A private, local-first journal for fathers. A father keeps a profile for each of his children and writes notes about them. He can choose to share some notes with a child, who opens them in the same app. Father and child can message each other.

> Status: **agreed**. Open: the father's cloud backup design and the Turso dev setup (section 10). Phases 0–5 are done; Phase 6 (command palette and polish) is in review.

---

## 1. Goals and non-goals

**Goals**
- A fast, minimal, elegant desktop app: Linux and macOS first, Windows later.
- Local-first. The father's vault lives on his machine (Turso) and works fully offline.
- Notes are Markdown text and/or images. The father makes his own labels, and assigns notes to one or more children.
- Optional sharing. Each child sees **only** notes the father explicitly shared with them.
- Children read shared notes, export them to PDF, and message their father.
- Themes, a settings panel (font, text size and more), and an optional Vim mode (**off by default**).
- A learning project. Each phase introduces a few Rust, GPUI, gpui-kit and Turso ideas at a time.
- A portfolio piece: clean architecture, tests, CI, docs and a polished demo that a potential employer can read and run (section 11).

**Scale: start small, built to grow**
- **Today:** one father (the owner), his children, maybe a few friends.
- **Later:** possibly open to anyone.
- **Approach:** the system is **multi-tenant from day one** (accounts, per-family isolation, versioned API), but it is *operated* small:
  - one cheap server,
  - Turso Cloud's free or entry tier,
  - sign-up by **invite code only**.
- **Growing later** is a config and operations change, not a rewrite.

**Non-goals (for v1)**
- Mobile apps, a web client, real-time collaborative editing.
- Multiple fathers or guardians editing one vault. The schema leaves room for it later.
- Push notifications. Messages arrive when the app polls or syncs.

---

## 2. The big decisions

### 2.1 Storage: the `turso` crate, locally
- **Crate:** `turso` (the Rust rewrite of SQLite). Latest stable is 0.8.x.
- **Why it fits:** pure Rust (no C toolchain), async, and it has built-in sync with Turso Cloud. The sync is what makes sharing simple.
- **Maturity:** pre-1.0, so expect some rough edges.
- **Experimental features:** encryption at rest and full-text search are still experimental. We use both behind settings and keep a plain fallback (a `LIKE` search, an unencrypted file).
- **Risk control:** all database access sits behind a `Store` type in `papa-core`. The UI never touches SQL. If Turso changes (Supabase announced in Oct 2026 that it is acquiring Turso), only one module has to change.
- **Backups:** the vault is a single file. We add "Export backup", because Turso's own FAQ advises keeping independent backups while it is pre-1.0.

### 2.2 Sharing and the cloud: one database per child, end-to-end encrypted

Turso Cloud has no row-level security. Its recommended pattern is **one database per tenant**, with **tokens scoped to a single database**. That fits our rule that "a child only sees their own notes" exactly:

```diagram
┌──────────── Father's machine ────────────┐        ┌──────── papa-relay (small Rust server) ────────┐
│ vault.db (local, private, full data)     │        │ father accounts (email + password hash)        │
│        │ encrypt the shared subset        │ ─────▶ │ creates the per-child cloud DBs (Platform API) │
│        ▼                                  │ login  │ issues short-lived, DB-scoped tokens           │
│ shares/<child>.db (local replica)        │ ◀───── │ never sees plaintext                            │
└──────────────│───────────────────────────┘ tokens └─────────────────────────────────────────────────┘
               │ push / pull (turso sync)
               ▼
┌──────────── Turso Cloud ─────────────────┐
│ share-<uuid>   (one DB per shared child) │
│   shared_note / shared_blob : ciphertext │
│   message                    : ciphertext│
└──────────────▲───────────────────────────┘
               │ pull notes, push messages
┌──────────────┴── Child's machine ────────┐
│ share.db (local replica, read-only notes)│
│ decrypted in memory → view / PDF export  │
└──────────────────────────────────────────┘
```

- **Why a relay server is needed:**
  - A desktop app can't safely contain Turso's org-level or Platform API tokens.
  - "Log in with Dad's email and my key" needs something to check the credentials.
  - The relay is small, about 5 endpoints, and is itself a nice Rust learning piece (axum + tokio).
- **End-to-end encryption:**
  - Everything in the cloud is ciphertext: titles, bodies, labels, images, messages. Only ids, sizes and timestamps are visible.
  - So a breach of Turso or the relay doesn't expose anything a father wrote about his kids.
- **Child access key** (the only child credential in v1; children don't set their own password):
  - The app generates a strong key per child, e.g. `PAPA-7K3Q-M9XD-…`, about 128 bits. The father hands it over in person or by text.
  - The child signs in with **father's email + access key**.
  - From the key, the app derives two separate keys with Argon2id followed by HKDF:
    - an `auth_key`, sent to the relay to get a token,
    - an `enc_key`, which never leaves the device and unwraps the share's content key.
- **Revoking or rotating:** the father can revoke a share, which deletes the cloud DB and its tokens, or regenerate the key. Regenerating re-encrypts the share with a new content key.
- **Permissions:**
  - Child tokens are read-only on notes, with insert-only rights on `message` (Turso's fine-grained table permissions).
  - Father tokens have full access to that share DB.
- **Sync conflicts:** Turso sync uses last-push-wins. That is fine here: notes only flow father → child, and messages are append-only rows with UUIDv7 ids, so nothing conflicts.
- **Images:**
  - Downscaled before sharing (longest edge ≤ 2048 px), then stored encrypted as BLOBs.
  - Large originals stay only in the father's local vault.
  - If sizes ever become a problem, move shared images to object storage (R2/S3) and keep only references in the database.

**Hosting ✅ decided: one central relay, run by the owner, invite-only.**
- **Who runs it:** the owner runs a single `papa-relay` instance (a small VPS or Fly.io) backed by one Turso Cloud organization.
- **Who can join:** a new father needs an **invite code**, minted with an admin command (`papa-relay admin invite create`). Opening sign-ups later is a setting.
- **Built for many users from the start:**
  - Every table is keyed by account.
  - Share databases get random names.
  - The API is versioned (`/v1/...`).
  - Request and response types live in `papa-proto`.
  - There is rate limiting and structured logging (`tracing`).
  - Config comes from environment variables.
  - The Turso Platform token lives only on the server.
- **Self-hosting stays possible but isn't built yet:**
  - The app reads the relay URL from settings.
  - The relay URL is part of the child's access key, so children never type it.
  - A "use my own server" option could be added later without changing the design.
- **Before inviting friends:** a short privacy policy and an "export / delete my account" function. The relay stores emails and metadata even though content is encrypted.

**Considered and rejected:**
- *A single shared cloud database for all users.* No row-level security, so isolation would rest entirely on our own server code.
- *No server, with the whole connection bundled into the access key.* The child login couldn't use the father's email, and the key couldn't be rotated or revoked cleanly.

### 2.3 Concurrency model
- **The problem:** GPUI has its own foreground and background executors. `turso` (and especially its `sync` feature) expects tokio.
- **The fix:** `papa-core` runs a dedicated tokio runtime on a background thread, the **database actor**. The UI sends it commands over an async channel and awaits replies inside `cx.spawn(...)`.
- **Result:** the UI thread never blocks on I/O. This is also a good lesson in channels, `Send`/`'static` bounds and ownership across threads.

### 2.4 PDF export
- **Approach:** turn the note's Markdown into Typst markup, then compile it to PDF in-process with the `typst` + `typst-pdf` crates. The fonts are bundled with the app.
- **Why Typst:** typeset output looks good (a keepsake a child may print) and supports images, headings and lists.
- **Cost:** Typst is heavy to compile. It gets its own crate (`papa-pdf`) so the rest of the workspace still builds fast.
- **Fallback if Typst is too heavy:** `krilla`, which is lower-level and means doing more layout by hand.

### 2.5 Dependencies policy ✅ decided
**gpui-kit is the only UI dependency**, and only the `papa` app crate depends on it. Never add gpui / gpui_platform / gpui-component separately. Non-UI crates (storage, crypto, serialization, PDF, HTTP) live in `papa-core`, `papa-pdf`, `papa-relay` and `papa-proto`, and are added deliberately. AGENTS.md has been updated.

---

## 3. Workspace layout

```
papa/
├── Cargo.toml                 # [workspace]
├── PLAN.md
├── README.md
├── docs/                      # architecture.md, adr/ (decision records)
├── assets/
│   ├── themes/*.json          # curated themes (from gpui-kit's themes/ + our own "Papa Paper")
│   └── fonts/                 # bundled UI + reading fonts (OFL licensed)
└── crates/
    ├── papa/                  # the desktop app (only crate depending on gpui-kit)
    │   └── src/
    │       ├── main.rs        # application().run, init, window
    │       ├── app.rs         # root view, routing between screens, global actions
    │       ├── state/         # app-level entities (session, selection, settings)
    │       ├── ui/            # sidebar, child_list, note_list, note_view, editor,
    │       │                  # labels, messages, settings, onboarding, child_mode
    │       ├── vim/           # optional modal editing layer
    │       └── theme.rs       # theme registry wiring
    ├── papa-core/             # no GPUI: models, Store (turso), migrations, crypto,
    │                          # share projection, relay client, settings file
    ├── papa-proto/            # serde request/response types shared by app + relay
    ├── papa-pdf/              # markdown → typst → pdf
    └── papa-relay/            # axum server: accounts, share DB provisioning, tokens
```

**Why split:** `papa-core` can be unit-tested without a window, compile times stay manageable, and the boundary between UI and logic is visible in the code.

---

## 4. Data model (local vault)

All ids are UUIDv7 (sortable by time). Rows are soft-deleted with `deleted_at`, so the share projection can propagate deletions.

| Table | Key columns |
|---|---|
| `profile` | id, display_name, email, avatar_image_id, created_at |
| `child` | id, name, birth_date, color, avatar_image_id, sort_order, archived_at |
| `label` | id, name, color, sort_order |
| `note` | id, title, body_md, pinned, created_at, updated_at, deleted_at |
| `note_child` | note_id, child_id — *many-to-many: one note can be about several children* |
| `note_label` | note_id, label_id |
| `image` | id, note_id, mime, width, height, sha256, created_at |
| `image_data` | image_id, kind (thumb / display / original), mime, bytes — *one row per version; see [ADR 0009](docs/adr/0009-images-stored-in-the-vault.md)* |
| `share` | child_id, remote_db, wrapped_content_key, key_salt, status, last_synced_at |
| `note_share` | note_id, child_id, shared_at, remote_version — *which notes each child can see* |
| `message` | id, child_id, direction, body, created_at, read_at, remote_id |
| `schema_version` | migrations |

**Behaviour**
- A **note** is Markdown with zero or more attached images. An "image note" is just a note with images and a short caption. One editor, one viewer.
- **Assigning** a note to a child and **sharing** it are separate steps:
  - assigned means the note is *about* that child;
  - shared means that child can *see* it.
  - Sharing is off by default. A per-child setting "share new notes automatically" can turn it on.
- **Labels** are always private to the father. They are never sent to the cloud, and shared notes don't carry them.

**Cloud share DB** (one per shared child). All payloads are encrypted with XChaCha20-Poly1305:
- `shared_note(id, ciphertext, nonce, updated_at, deleted)` — the ciphertext holds title, body and dates, never labels
- `shared_blob(id, note_id, ciphertext, nonce)`
- `message(id, sender, ciphertext, nonce, created_at)`
- `receipt(message_id, read_at)`

---

## 5. UX sketch

**Father mode**
```diagram
┌──────────────────────────────────────────────────────────────────────┐
│ ● ● ●  Papa                                   ⌘K search   ◐ theme    │
├────────────┬──────────────────────┬──────────────────────────────────┤
│ CHILDREN   │ Emma · 24 notes      │ First bike ride            ✎ ⇪ ⋯ │
│ ● Emma     │ ┌──────────────────┐ │ Mar 3 2026 · #milestone · Emma   │
│ ● Leo      │ │ First bike ride  │ │                                  │
│ + Add      │ │ #milestone · 3d  │ │ She refused the training wheels… │
│            │ ├──────────────────┤ │ ┌──────┐ ┌──────┐                │
│ LABELS     │ │ School play      │ │ │ img  │ │ img  │                │
│ # milestone│ │ #school · 2w     │ │ └──────┘ └──────┘                │
│ # funny    │ └──────────────────┘ │ Shared with Emma ✓               │
│            │                      │                                  │
│ ✉ Messages │                      │                                  │
│ ⚙ Settings │                      │                                  │
├────────────┴──────────────────────┴──────────────────────────────────┤
│ NORMAL  ·  synced 2 min ago                                          │  ← status bar (Vim mode shown only if enabled)
└──────────────────────────────────────────────────────────────────────┘
```
- **Three panes:** Sidebar → note list (filter by child, label or search) → note view/editor. Built with the `Sidebar`, `Resizable`, `VirtualList`, `TextView::markdown`, `Editor` and `TitleBar` components.
- **Editor:** Edit and Preview toggle with ⌘E, plus an optional side-by-side split. Autosaves while you type, in the background.
- **Command palette (⌘K):** jump to a child, note or label, or run any action. Everything is reachable from the keyboard.
- **Images:** add with drag and drop, paste, or a file picker. Thumbnails are generated in the background.
- **Sharing:** a "Share with…" popover per note. The child-profile page shows the share status, the access key (copy / regenerate / revoke) and the last sync.

**Child mode** (same binary)
- **First launch** asks: *"Start my own vault"* or *"Open notes shared with me"*.
- **Sign in:** with Dad's email and the access key.
- **Child view:** the same layout, read-only, with a single child and no editor. Children can't comment on notes; they talk to Dad only through Messages. It shows the note list, note view, Messages, Settings and **Export PDF** (one note, a selection, or everything).

**Messages**
- A chat thread per child, built with the `MessageGroup` and `Bubble` components.
- The app polls every 30–60 s while open, and immediately after you send a message. The sidebar shows an unread badge.

---

## 6. Themes, settings, Vim

**Themes**
- **Bundled themes:** a curated set of gpui-kit's theme JSONs, plus a signature warm "Papa Paper" light theme and a matching dark one. They are embedded with `include_str!` and loaded into `ThemeRegistry`.
- **User themes:** the app watches a user themes directory (`~/.config/papa/themes`) with `ThemeRegistry::watch_dir`, so editing a theme hot-reloads it.
- **Mode:** Light, Dark, or follow the system.

**Settings**
- Stored in `~/.config/papa/settings.json`. This is **separate from the vault**, so the window looks right before the vault is unlocked.
- Built with gpui-kit's `Settings → SettingPage → SettingGroup → SettingItem` components (which include search).
- **Appearance:** theme, mode, UI font, reading font, editor (mono) font, base text size, line height, compact/comfortable density.
- **Editor:** Vim mode (off), autosave delay, spellcheck (later), show the Markdown preview by default.
- **Privacy:** lock the vault with a password (turns on Turso encryption at rest), auto-lock after N minutes idle.
- **Sync:** relay URL, sync interval, "sync now".
- **Data:** vault location, export/import backup.

**Vim mode**
- gpui-kit has no Vim support, so we build a small layer, `papa::vim`, on top of the editor's public cursor, selection and insert API.
- **Modes:** Normal, Insert, Visual (characterwise).
- **Motions:** `h j k l w b e 0 ^ $ gg G`
- **Entering Insert mode:** `i a I A o O`
- **Edits:** `x dd yy p P u Ctrl-r`, plus `d`/`c`/`y` with a motion, and `.` later.
- **Commands:** `:w` saves, `:q` closes the note, `Esc` returns to Normal.
- **Lists:** `j`/`k` also move through the note list when Vim mode is on.
- **Mode indicator:** shown in the status bar.
- **Off-switch:** when Vim mode is off, the layer is never attached, so it adds zero overhead.

---

## 7. Security summary
- **Local vault:**
  - Optionally encrypted at rest with Turso's encryption (AEGIS-256).
  - The key comes from the father's vault password via Argon2id.
  - The key stays in memory only and is zeroized when the vault locks.
  - Optionally stored in the OS keychain to unlock automatically.
- **Cloud:**
  - End-to-end encrypted.
  - The relay stores only Argon2id hashes of father passwords and child `auth_key`s.
  - Tokens are short-lived and scoped to one database.
- **Crates (proposed):** `argon2`, `hkdf`, `sha2`, `chacha20poly1305`, `rand`, `zeroize`, and later `keyring`.
- **Unrecoverable keys:** a lost father password can't be recovered (by design). A lost child key is fixed by having Dad regenerate it.

---

## 8. Phases — one step at a time

Each phase ends with something runnable. Each one has a short "what you'll learn" list, so the code doubles as study material.

| # | Phase | Delivers | You'll learn |
|---|---|---|---|
| 0 ✅ | **Foundations** | Cargo workspace, crate split, CI (build + clippy on Linux/macOS) | workspaces, features, `cargo` profiles |
| 1 ✅ | **App shell** | Window, `Root`, custom `TitleBar`, Sidebar + 3 resizable panes with placeholder data, Light/Dark toggle, first actions + keybindings | `Entity`, `Render` vs `RenderOnce`, `Context`, actions, key contexts, the gpui-kit layout helpers (`h_flex`/`v_flex`) |
| 2 ✅ | **Local storage** | `papa-core::Store` on `turso`, migrations, the database actor thread, CRUD for children and labels wired into the UI | async Rust, tokio vs GPUI executors, channels, `thiserror`/`anyhow`, testing with a temporary database |
| 3 ✅ | **Notes** | Note list (virtualized), Markdown editor + preview, autosave, assign to children, labels, filtering, search | `EditorState`, subscriptions/events, debouncing with tasks, `TextView` |
| 4 ✅ | **Images** | Attach with drag and drop/paste/picker, thumbnails in the background, gallery and lightbox | `image` crate, background tasks, `img()` / `ImageSource`, file dialogs |
| 5 ✅ | **Settings & themes** | Settings panel, settings.json, font/size/density, bundled + user themes with hot reload | serde, globals (`cx.set_global`), `ThemeRegistry`, observing settings changes |
| 6 | **Command palette & polish** | ⌘K palette, empty states, animations, keyboard navigation everywhere | `Command`, focus handles, motion |
| 7 | **Vim mode** | Normal/Insert/Visual, the motions and edits listed above, status-bar indicator, list navigation | state machines with enums, key event handling, a test-driven parser |
| 8 | **PDF export** | `papa-pdf`: Markdown → Typst → PDF, with images; export one note, a selection or a child's whole book | the `typst` `World` trait, AST walking, crate isolation |
| 9 | **Vault lock & backup** | Optional at-rest encryption, password unlock screen, auto-lock, export/import backup | Argon2id, zeroize, Turso encryption |
| 10 | **Relay server** | `papa-relay`: father accounts, share-DB provisioning via the Turso Platform API, token minting; local dev mode with a local Turso and no cloud | axum, tokio, HTTP API design, `papa-proto` shared types |
| 11 | **Sharing (father side)** | Generate/rotate/revoke keys, encrypted projection of shared notes into `shares/<child>.db`, push via turso sync | key derivation, AEAD, idempotent sync design |
| 12 | **Child mode** | Onboarding choice, sign in, pull + decrypt, read-only UI, PDF export | the app's two "modes" as types, offline caching |
| 13 | **Messaging** | Chat per child both ways, polling, unread badges, read receipts | append-only sync, optimistic UI |
| 14 | **Packaging** | `cargo-packager`: `.deb` + AppImage for Linux, `.app` + `.dmg` for macOS (signing/notarization when you have a dev account); later Windows MSI | release profiles, bundling assets, CI releases |
| 15 | **Showcase** | `papa --demo` with sample data (no real family data in screenshots), screenshots + short video, polished README, architecture write-up, downloadable releases | presenting technical work |

Phases 0–9 give a complete, useful, **offline** app before any cloud work starts. Cloud work (10–13) can slip without blocking anything else.

---

### Where each Turso feature is learned
| Turso feature | Phase |
|---|---|
| Embedded local database, migrations, async API | 2 |
| Full-text search (experimental `fts`) | evaluated in 3, deferred ([ADR 0008](docs/adr/0008-note-search-without-turso-fts.md)) |
| Encryption at rest | 9 |
| Platform API, database-per-tenant, fine-grained scoped tokens | 10 |
| Embedded sync: push/pull, offline-first replicas | 11–13 |
| (stretch) `BEGIN CONCURRENT` / MVCC, change data capture for the share projection | 11+ |

## 9. Risks
- **`turso` is pre-1.0.**
  - Pin exact versions and update them deliberately.
  - Keep the `Store` abstraction, back up the vault, and gate the experimental features.
- **Turso / Supabase acquisition.** Pricing and plans may change. The relay isolates provisioning, so moving to another backend means changing one crate.
- **gpui / gpui-kit move fast** (pre-1.0, frequent breaking changes). Pin `gpui-kit` and upgrade in dedicated steps.
- **Hosting the relay.** Someone has to run it (Fly.io, Railway or a small VPS). Turso's free tier limits on database counts apply. For learning and development, a local relay + local Turso is enough.
- **Typst compile size.** Isolated in `papa-pdf`; fall back to `krilla` if needed.

---

## 10. Decisions and open topics

**Decided**
1. Dependencies: gpui-kit is the only UI dependency (section 2.5).
2. One note can be assigned to **several children**.
3. Labels are **always private** to the father.
4. Children can **only send messages**; they can't comment on notes.
5. A generated **access key is the only child credential** in v1.
6. The product name stays **Papa**.

7. **Hosting:** one central relay run by the owner, **invite-only**, multi-tenant by design (section 2.2). Self-hosting stays possible but isn't built yet.
8. **Scale:** for the owner, his children and a few friends first; built so it can grow.
9. **Public repository**, built in public. License: dual **MIT OR Apache-2.0** (the Rust ecosystem convention, and what gpui-kit uses) unless the owner prefers otherwise. No secrets ever in the repo (section 12).

**Still to discuss**
- **Father's encrypted cloud backup:** wanted, scheduled after Phase 13. The design is still open (an encrypted snapshot is the likely first step).
- **Turso dev setup:** confirm whether local development of sync needs a Turso Cloud dev database or can run fully locally. A short test at the start of Phase 10 settles this.

---

## 11. Portfolio practices (from Phase 0 onwards)
- **README:** what Papa is, screenshots, the architecture diagram, how to build, and a "what I learned" section.
- **`docs/adr/`:** short Architecture Decision Records for the big choices in section 2 (Turso, one database per child, end-to-end encryption, Typst, invite-only relay). Interviewers love these, and they record *why*, not just *what*.
- **`docs/architecture.md`:** crates, data flow, the threading model and the security model.
- **CI badges:** build, clippy (`-D warnings`), `cargo fmt --check` and tests on Linux and macOS.
- **Tests where they count:** `papa-core` (store, crypto round-trips, share projection), the Vim parser, and relay API integration tests.
- **Commits:** small and readable, one feature per branch, PRs with a description even when working solo.
- **Demo mode:** `papa --demo` loads sample children and notes, so screenshots never contain real family data.
- **Releases:** tagged versions with Linux and macOS downloads (Phase 14).

---

## 12. Secrets hygiene (public repo)
- **No secrets in code or config files.**
  - The relay reads everything sensitive from environment variables: Turso Platform token, org name, signing keys, admin credentials.
  - `.env.example` documents the variable names with dummy values. The real `.env` is gitignored.
- **Gitignored from Phase 0:** `.env*` (except `.env.example`), `*.db`, `*.db-wal`, `*.db-shm`, `shares/`, local vault folders, and `learning/` (already ignored).
- **Secret scanning:**
  - `gitleaks` runs in CI on every push and PR.
  - GitHub secret scanning and push protection are enabled on the repo.
  - Optionally, `gitleaks` also runs as a local pre-commit hook.
- **CI and deploy secrets** live only in GitHub Actions secrets and the hosting provider's secret store (e.g. `fly secrets set`), never in workflow files.
- **No real family data:**
  - Test fixtures, screenshots and the demo dataset use invented children and notes (`papa --demo`).
  - Real vaults live in the OS data directory, outside the repo.
- **Data stays safe even if the server leaks:** everything in the cloud is end-to-end encrypted, so a leaked database or token exposes only ciphertext. Tokens are short-lived and scoped to one database, which limits the damage further.
- **If a secret ever leaks:** rotate it first, then clean the git history. Rotating is what actually protects you; history rewrites don't remove copies that were already cloned.

