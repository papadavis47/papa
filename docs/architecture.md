# Architecture

This is the living overview of how Papa fits together. [PLAN.md](../PLAN.md) has the full design and roadmap. The *why* behind each major choice is in the [decision records](adr/).

## Crates

```
papa (desktop app, gpui-kit) ──depends on──▶ papa-core ──▶ papa-proto
                             └─depends on──▶ papa-pdf
papa-relay (server, axum)    ──depends on──▶ papa-proto
```

- **papa** is the only crate that depends on `gpui-kit`. It holds the views, actions, keybindings, themes and Vim layer.
- **papa-core** has no UI code. It owns the models, the `Store` (local Turso database and migrations), crypto, the share projection and the settings file. It is unit-tested without a window.
- **papa-proto** holds the serde types for the relay API. The app and the server compile against the same definitions.
- **papa-pdf** turns Markdown into PDF. It is kept separate because the PDF toolchain is heavy to compile.
- **papa-relay** is a small HTTP service: father accounts, invite codes, creating one cloud database per shared child, and issuing short-lived, database-scoped tokens.

## Desktop app

```diagram
┌─ PapaApp (Entity, Render) ─────────────────────────────────────────┐
│ owns: Vault (Opening / Open(Store) / Failed), children, labels,    │
│       Selection, sidebar_visible, focus                            │
│                                                                    │
│ AppTitleBar ── sidebar toggle · "Papa" · theme toggle              │
│ h_resizable ┬─ AppSidebar   key context "ChildList"                │
│             │               children, labels (right-click: edit,   │
│             │               delete), Messages, Settings            │
│             ├─ NoteList     selected child (notes arrive Phase 3)  │
│             └─ NoteView                                            │
│ StatusBar  ── vault path · theme                                   │
│ dialogs    ── add/edit child, add/rename label, confirm delete     │
└────────────────────────────────────────────────────────────────────┘
```

- **State lives in entities, presentation in components.** `PapaApp` is the only long-lived stateful view. The pieces in `crates/papa/src/ui/` are `RenderOnce` components: built each frame from data passed in, consumed when rendered. They report clicks back through callbacks the parent creates with `cx.listener`. The one other entity is `NameForm`, which holds a dialog's half-typed input for as long as the dialog is open.
- **One event enum for the sidebar.** `AppSidebar` reports everything (select, add, edit, delete) as a `SidebarEvent` through a single callback, and `PapaApp` handles it in one `match`.
- **Selection logic is plain Rust** (`state.rs`), unit-tested without a window.
- **The UI shows copies of the vault's rows.** After each change `PapaApp` reloads the affected list from the store, so the screen always matches what's stored. Failures appear as notifications and leave the state unchanged.
- **Actions** are the single entry point for commands. A keyboard shortcut and a button dispatch the same action:
  - App-level actions (`Quit`, `ToggleTheme`) are handled with `cx.on_action`, because they act on global state.
  - View-level actions (`ToggleSidebar`, `SelectNextChild`, `SelectPreviousChild`) are handled by `PapaApp`, because they change its state.
- **Key contexts** scope bindings. `PapaApp` bindings work anywhere in the window. `ChildList` bindings (↑/↓) work only while the sidebar has focus, so text fields keep their arrow keys.
- **Window chrome:** the app draws its own title bar. On Linux it asks for client-side decorations so the compositor doesn't add a second one.

| Shortcut (⌘ on macOS, Ctrl elsewhere) | Action |
|---|---|
| ⌘Q | Quit |
| ⌘⇧L | Toggle light/dark |
| ⌘B | Toggle sidebar |
| ↑ / ↓ (sidebar focused) | Previous / next child |
| Enter / Esc (in a dialog) | Save / cancel |

## Local storage

The vault is one Turso (SQLite-format) file: `vault.db` in the OS data directory, or the path in `PAPA_VAULT`. `papa-core` owns it; the app never sees SQL.

```diagram
 UI thread (GPUI)                          papa-store thread (tokio, current-thread)
┌──────────────────────────────┐          ┌───────────────────────────────────────┐
│ cx.spawn_in(async {          │ Request  │ while let Some(req) = inbox.recv() {  │
│   store.add_child(d).await ──┼─────────▶│   db.add_child(d).await  ── SQL ──┐   │
│ })                           │  mpsc    │   reply.send(result)              │   │
│        ▲                     │          │ }                                 ▼   │
│        └── result ◀──────────┼──────────┤                          vault.db     │
└──────────────────────────────┘ oneshot  └───────────────────────────────────────┘
```

- **`Store`** is the public handle: a cloneable channel sender. Each method sends a `Request` with a reply channel and awaits the answer, so the UI thread never blocks. See [ADR 0007](adr/0007-database-actor-thread.md).
- **`store::db`** is the only module with SQL. It validates input (names are trimmed, 1–60 characters; label names are unique ignoring case) and returns typed `papa_core::Error`s.
- **Migrations** are numbered `.sql` files embedded in the binary. The schema version is stored in SQLite's `user_version` header field. Each missing migration runs in one transaction together with its version bump, so a crash can't leave a half-migrated vault. A vault from a newer Papa is refused rather than modified.
- **Errors:** `papa-core` uses `thiserror` enums that callers can match on. The app uses `anyhow` where it only reports errors, adding context such as which file it was opening.

| Table | Columns | Since |
|---|---|---|
| `child` | `id` (UUIDv7 text), `name`, `color` (0xRRGGBB), `sort_order` | v1 |
| `label` | `id` (UUIDv7 text), `name` (unique, `COLLATE NOCASE`) | v1 |

## Data flow (planned)

1. The father's vault is a local Turso database. It is the source of truth and works offline.
2. When a note is shared with a child, it is encrypted on the father's machine. It is then written to a local replica of that child's share database and pushed to Turso Cloud.
3. The child's app signs in through the relay (father's email + access key). It receives a read-only token scoped to its own database, pulls, and decrypts locally.
4. Messages are append-only encrypted rows in the same share database. Both sides push and pull them.

## Threading

- **UI thread:** GPUI's executors. Long work is awaited in GPUI tasks, never run inline.
- **`papa-store` thread:** owns the database connection and runs a single-threaded tokio runtime (tokio is also what `turso`'s sync needs later). It exits when the last `Store` handle is dropped.

## Security model (planned)

- Optional encryption at rest for the local vault, keyed from the father's password (Argon2id).
- End-to-end encryption for everything in the cloud (XChaCha20-Poly1305). The relay and Turso only see ciphertext, ids and timestamps.
- Tokens are short-lived and scoped to a single database. No admin or Platform credentials ship in the app.
