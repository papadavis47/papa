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
│ owns: Journal (sample data), Selection, sidebar_visible, focus     │
│                                                                    │
│ AppTitleBar ── sidebar toggle · "Papa" · theme toggle              │
│ h_resizable ┬─ AppSidebar   children, labels, Messages, Settings   │
│             ├─ NoteList     key context "NoteList"                 │
│             └─ NoteView                                            │
│ StatusBar                                                          │
└────────────────────────────────────────────────────────────────────┘
```

- **State lives in entities, presentation in components.** `PapaApp` is the only stateful view. The pieces in `crates/papa/src/ui/` are `RenderOnce` components: built each frame from data passed in, consumed when rendered. They report clicks back through callbacks the parent creates with `cx.listener`.
- **Selection logic is plain Rust** (`state.rs`), unit-tested without a window.
- **Actions** are the single entry point for commands. A keyboard shortcut and a button dispatch the same action:
  - App-level actions (`Quit`, `ToggleTheme`) are handled with `cx.on_action`, because they act on global state.
  - View-level actions (`ToggleSidebar`, `SelectNextNote`, `SelectPreviousNote`) are handled by `PapaApp`, because they change its state.
- **Key contexts** scope bindings. `PapaApp` bindings work anywhere in the window. `NoteList` bindings (↑/↓) work only while the note list has focus, so a future editor keeps its arrow keys.
- **Window chrome:** the app draws its own title bar. On Linux it asks for client-side decorations so the compositor doesn't add a second one.

| Shortcut (⌘ on macOS, Ctrl elsewhere) | Action |
|---|---|
| ⌘Q | Quit |
| ⌘⇧L | Toggle light/dark |
| ⌘B | Toggle sidebar |
| ↑ / ↓ (note list focused) | Previous / next note |

## Data flow (planned)

1. The father's vault is a local Turso database. It is the source of truth and works offline.
2. When a note is shared with a child, it is encrypted on the father's machine. It is then written to a local replica of that child's share database and pushed to Turso Cloud.
3. The child's app signs in through the relay (father's email + access key). It receives a read-only token scoped to its own database, pulls, and decrypts locally.
4. Messages are append-only encrypted rows in the same share database. Both sides push and pull them.

## Threading (planned)

GPUI runs its own executors. The `turso` crate is async and its sync feature expects tokio. `papa-core` therefore runs the database on a dedicated tokio runtime thread, an actor. The UI sends it commands over a channel and awaits replies in GPUI tasks, so the UI thread never blocks on I/O.

## Security model (planned)

- Optional encryption at rest for the local vault, keyed from the father's password (Argon2id).
- End-to-end encryption for everything in the cloud (XChaCha20-Poly1305). The relay and Turso only see ciphertext, ids and timestamps.
- Tokens are short-lived and scoped to a single database. No admin or Platform credentials ship in the app.
