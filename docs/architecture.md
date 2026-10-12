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
│       notes (the list), Selection (filter + open note), search box │
│                                                                    │
│ AppTitleBar ── sidebar toggle · "Papa" · theme toggle              │
│ h_resizable ┬─ AppSidebar   key context "Sidebar"                  │
│             │               All notes, children, labels (click to  │
│             │               filter; right-click: edit, delete)     │
│             ├─ NoteList     key context "NoteList"                 │
│             │               heading + count, search, uniform_list  │
│             └─ NoteEditor   (Entity) title, Markdown body or       │
│                             preview, children/labels popover,      │
│                             autosave ── emits Saved / Deleted ──▶  │
│ StatusBar  ── vault path · theme                                   │
│ dialogs    ── add/edit child, add/rename label, confirm delete     │
└────────────────────────────────────────────────────────────────────┘
```

- **State lives in entities, presentation in components.** `PapaApp` owns the app's state. Its methods are split by pane: `app/sidebar.rs` and `app/notes.rs`. The pieces in `crates/papa/src/ui/` are mostly `RenderOnce` components: built each frame from data passed in, consumed when rendered. They report clicks back through callbacks the parent creates with `cx.listener`. Two entities have state of their own:
  - `NameForm` holds a dialog's half-typed input for as long as the dialog is open.
  - `NoteEditor` holds the open note, its text fields and its pending save.
- **Events between entities.** `NoteEditor` implements `EventEmitter<NoteEditorEvent>`. It emits `Saved(Note)` after every save, including changes of children or labels, and `Deleted(NoteId)`. `PapaApp` subscribes with `cx.subscribe_in` and patches the list row in place rather than reloading the list, so a note doesn't vanish from under the cursor when its new title stops matching the search. The editor in turn subscribes to its text fields' `InputEvent::Change`.
- **Autosave is debounced with a task.** Each change replaces `autosave: Option<Task<()>>` with a task that waits 500 ms, then saves. Dropping a GPUI task cancels it, so only the last keystroke's timer fires. A pending save is flushed at once when another note opens, and `cx.on_app_quit` awaits one last save before the app exits. The search box is debounced the same way (150 ms).
- **Saves don't depend on the editor.** A save reads the text when it starts and owns its strings and its `Store` clone, so it completes even if the editor has moved on to another note.
- **One event enum per component.** `AppSidebar` reports everything as a `SidebarEvent` and `NoteList` as a `NoteListEvent`, each through a single callback. `PapaApp` handles each in one `match`.
- **Selection logic is plain Rust** (`state.rs`): the filter (all notes, a child or a label), the open note, ↑/↓ movement, and what to open when the list changes. It is unit-tested without a window.
- **The note list is virtualized.** `uniform_list` asks only for the rows in view, so 5,000 notes draw as fast as 5. The list data is an `Rc<Vec<NoteSummary>>`: the row builder outlives the frame, and cloning an `Rc` is cheap.
- **The UI shows copies of the vault's rows.** After each change `PapaApp` reloads the affected list from the store, so the screen always matches what's stored. Failures appear as notifications and leave the state unchanged. Assigning a child or label is the one optimistic update: the checkbox flips at once, and the stored note replaces it when it arrives.
- **Actions** are the single entry point for commands. A keyboard shortcut and a button dispatch the same action:
  - App-level actions (`Quit`, `ToggleTheme`) are handled with `cx.on_action`, because they act on global state.
  - View-level actions (`ToggleSidebar`, `NewNote`, `TogglePreview`, `FocusSearch`, and moving through filters and notes) are handled by `PapaApp`, because they change its state.
- **Key contexts** scope bindings:
  - `PapaApp` bindings work anywhere in the window.
  - `Sidebar` and `NoteList` bindings (↑/↓) work only while that pane has focus, so text fields keep their arrow keys.
  - A focused text field handles ↑/↓ itself. The binding on `NoteSearch > Input` is just as specific and registered later, so it wins: ↑/↓ move through the notes while the search box keeps focus.
  - If focus disappears (a dialog closes and the element that had focus before it is gone), `cx.on_focus_lost` puts it back on the root, so the window-wide shortcuts keep working.
- **Icons:** gpui-kit's default asset source embeds only the components' icons. `assets.rs` adds the extra Lucide icons the app uses with `icon_assets!` and combines both into one `AssetSource`.
- **Window chrome:** the app draws its own title bar. On Linux it asks for client-side decorations so the compositor doesn't add a second one.

| Shortcut (⌘ on macOS, Ctrl elsewhere) | Action |
|---|---|
| ⌘Q | Quit (saves the open note first) |
| ⌘⇧L | Toggle light/dark |
| ⌘B | Toggle sidebar |
| ⌘N | New note for the selected child or label |
| ⌘E | Switch the note between Edit and Preview |
| ⌘F | Jump to search; Esc clears it |
| ↑ / ↓ (sidebar focused) | Previous / next entry |
| ↑ / ↓ (note list or search focused) | Previous / next note |
| Enter (in the title) | Move to the body |
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

- **Notes** belong to any number of children and labels through two link tables. Their foreign keys cascade, so deleting a child or label unassigns its notes but never deletes them. Changing a note's children or labels replaces the set in one transaction.
- **Deleted notes become tombstones:** the text is wiped, the links removed and `deleted_at` set, as PLAN.md §4 asks. Sharing (Phase 11) can then tell a child's app the note is gone. Every query skips them, helped by a partial index.
- **Search** runs in Rust on the database thread: every word must appear in the title or body, ignoring case (Unicode-aware, unlike SQL's `lower()`). Turso's experimental full-text search was evaluated and deferred; see [ADR 0008](adr/0008-note-search-without-turso-fts.md).
- **The write-ahead log:** Turso leaves recent commits in `vault.db-wal` after the app quits; it doesn't checkpoint on close like SQLite does. Nothing is lost, but a backup must checkpoint first or copy both files (Phase 9).

| Table | Columns | Since |
|---|---|---|
| `child` | `id` (UUIDv7 text), `name`, `color` (0xRRGGBB), `sort_order` | v1 |
| `label` | `id` (UUIDv7 text), `name` (unique, `COLLATE NOCASE`) | v1 |
| `note` | `id`, `title`, `body_md` (Markdown), `created_at`, `updated_at`, `deleted_at` (Unix ms) | v2 |
| `note_child` | `note_id`, `child_id` (both cascade on delete) | v2 |
| `note_label` | `note_id`, `label_id` (both cascade on delete) | v2 |

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
