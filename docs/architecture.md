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
│ AppTitleBar ── sidebar toggle · "Papa" · ⌘K search · theme toggle   │
│ h_resizable ┬─ AppSidebar   key context "Sidebar"                  │
│             │               All notes, children, labels (click to  │
│             │               filter; right-click: edit, delete)     │
│             ├─ NoteList     key context "NoteList"                 │
│             │               heading + count, search, uniform_list  │
│             └─ NoteEditor   (Entity) title, Markdown body or       │
│                             preview, children/labels popover,      │
│                             autosave ── emits Saved / Deleted /    │
│                             ImagesChanged ──▶                      │
│                └─ Gallery   (Entity) thumbnail strip; adds images  │
│                             from the picker, a drop or a paste     │
│ StatusBar  ── vault path · theme name                              │
│ Lightbox   ── (Entity, owned by Gallery) one image over the whole  │
│               window; key context "Lightbox"                       │
│ CommandPalette ── (Entity) ⌘K over everything; key context         │
│               "CommandPalette"; runs actions, emits Run(command)   │
│ dialogs    ── add/edit child, add/rename label, confirm delete     │
└────────────────────────────────────────────────────────────────────┘

┌─ SettingsPanel (Entity, its own window) ───────────────────────────┐
│ gpui-kit Settings: Appearance · Editor · Data, with search         │
│ fields read and write the AppSettings global                       │
└────────────────────────────────────────────────────────────────────┘

 Globals: AppSettings ──observed by──▶ theme.rs ──rebuilds──▶ Theme
          ThemeRegistry ─observed by─▶ theme.rs     (every window
          (bundled + themes folder)                  reads Theme)
```

- **State lives in entities, presentation in components.** `PapaApp` owns the app's state. Its methods are split by pane: `app/sidebar.rs` and `app/notes.rs`. The pieces in `crates/papa/src/ui/` are mostly `RenderOnce` components: built each frame from data passed in, consumed when rendered. They report clicks back through callbacks the parent creates with `cx.listener`. Two entities have state of their own:
  - `NameForm` holds a dialog's half-typed input for as long as the dialog is open.
  - `NoteEditor` holds the open note, its text fields and its pending save.
  - `Gallery` holds the open note's images and the ones still being added.
  - `Lightbox` holds the image shown full size and the full-size versions loaded so far.
  - `CommandPalette` holds gpui-kit's `CommandState` (the search, the highlighted row) and the focus to give back when it closes.
- **Events between entities.** `NoteEditor` implements `EventEmitter<NoteEditorEvent>`. It emits `Saved(Note)` after every save, including changes of children or labels, and `Deleted(NoteId)`. `PapaApp` subscribes with `cx.subscribe_in` and patches the list row in place rather than reloading the list, so a note doesn't vanish from under the cursor when its new title stops matching the search. The editor in turn subscribes to its text fields' `InputEvent::Change`.
- **Autosave is debounced with a task.** Each change replaces `autosave: Option<Task<()>>` with a task that waits the autosave delay (a setting, 500 ms by default), then saves. Dropping a GPUI task cancels it, so only the last keystroke's timer fires. A pending save is flushed at once when another note opens, and `cx.on_app_quit` awaits one last save before the app exits. The search box is debounced the same way (150 ms).
- **Saves don't depend on the editor.** A save reads the text when it starts and owns its strings and its `Store` clone, so it completes even if the editor has moved on to another note.
- **One event enum per component.** `AppSidebar` reports everything as a `SidebarEvent` and `NoteList` as a `NoteListEvent`, each through a single callback. `PapaApp` handles each in one `match`.
- **The command palette** (⌘K, see [ADR 0011](adr/0011-command-palette-inside-the-main-view.md)) is gpui-kit's `Command`, drawn as an overlay inside `PapaApp`'s root:
  - **What it offers is plain Rust** (`palette.rs`): groups of rows (Actions, Go to, This note, Notes) built from a snapshot of the app's state each time it opens, unit-tested without a window. The notes are read from the vault on opening, since the list may be filtered; they join the palette when they arrive.
  - **Rows with a shortcut carry its action.** Choosing one dispatches the action from inside `PapaApp`'s tree, so the same handler runs as for the key, and the row shows the key bound to it. Other rows (go to, open a note, assign, add a child or label, delete) come back to `PapaApp` as a `CommandPaletteEvent` after the palette has closed and given focus back.
  - **It closes when focus leaves it** (`cx.on_focus_out`): a command that moves focus closes it. Esc first clears the search, then closes. A click outside closes it too.
  - **UI tests** drive it in a headless window with `#[gpui_kit::test]`: type, press Enter or Esc, then check what ran and where focus went.
- **Keyboard navigation.** ⌘1, ⌘2 and ⌘3 focus the sidebar, the note list and the note. → or Enter moves one pane right, ← or Esc one pane left. Enter in the search box opens the highlighted note. The selected entry of the focused pane has a focus ring: `PapaApp` asks `focus_handle.contains_focused` while rendering, and GPUI redraws the window whenever focus moves. Every entry has a transparent border, so the ring doesn't shift the text.
- **Key precedence does the routing.** When several bindings match a key, GPUI tries the most specific first, and if its handler lets the action propagate, the next. So Esc in the note's text field first does the field's own job (closing its menu, say), and only when it has none does the `NoteEditor` binding take the father back to the list.
- **Empty states** (`ui/empty_state.rs`, on gpui-kit's `Empty`) say why a pane is empty and offer the next step, with its shortcut: no notes yet, nothing about a child, nothing with a label, no search matches (with "Clear search"), and no note open (a short list of keys).
- **Motion.** The palette fades and drops in, a note fades in when it opens, and empty states fade and rise. Each is `with_animation(id, Animation, |element, t| …)`: GPUI renders a frame for every step of `t` from 0 to 1, shaped by an easing curve, and keeps the progress under the element's id. A new id replays it, so the palette's id counts openings and the note's id names the note (switching notes replays it, switching Edit and Preview doesn't). The "Reduce motion" setting sets GPUI's `App::reduce_motion`, which makes every animation, gpui-kit's dialogs included, draw its last frame at once.
- **Selection logic is plain Rust** (`state.rs`): the filter (all notes, a child or a label), the open note, ↑/↓ movement, and what to open when the list changes. It is unit-tested without a window.
- **The note list is virtualized.** `uniform_list` asks only for the rows in view, so 5,000 notes draw as fast as 5. The list data is an `Rc<Vec<NoteSummary>>`: the row builder outlives the frame, and cloning an `Rc` is cheap.
- **The UI shows copies of the vault's rows.** After each change `PapaApp` reloads the affected list from the store, so the screen always matches what's stored. Failures appear as notifications and leave the state unchanged. Assigning a child or label is the one optimistic update: the checkbox flips at once, and the stored note replaces it when it arrives.
- **Actions** are the single entry point for commands. A keyboard shortcut, a button and a palette row dispatch the same action:
  - App-level actions (`Quit`, `ToggleTheme`, `OpenSettings`) are handled with `cx.on_action`, because they act on global state.
  - View-level actions (`ToggleSidebar`, `NewNote`, `TogglePreview`, `FocusSearch`, `AddImages`, `ToggleCommandPalette`, `FocusSidebar`/`FocusNoteList`/`FocusEditor`, and moving through filters and notes) are handled by `PapaApp`, because they change its state.
  - The lightbox's actions (`CloseLightbox`, `NextImage`, `PreviousImage`) are handled by the `Lightbox` entity.
- **Key contexts** scope bindings:
  - `PapaApp` bindings work anywhere in the window.
  - `NoteEditor` (Esc goes back to the list) and `CommandPalette` bindings work while focus is inside them. In the palette's search box ⌘F is bound again, as `CommandPalette > Input`, to win over the text field's own ⌘F.
  - `Sidebar` and `NoteList` bindings (↑/↓) work only while that pane has focus, so text fields keep their arrow keys.
  - `Lightbox` bindings (←/→, Esc) work only while it's open: it takes focus when it opens and gives it back when it closes.
  - A focused text field handles ↑/↓ itself. The binding on `NoteSearch > Input` is just as specific and registered later, so it wins: ↑/↓ move through the notes while the search box keeps focus.
  - If focus disappears (a dialog closes and the element that had focus before it is gone), `cx.on_focus_lost` puts it back on the root, so the window-wide shortcuts keep working.
- **Images** (see [ADR 0009](adr/0009-images-stored-in-the-vault.md)):
  - **Adding** works three ways, all ending in `Gallery::add`: the file picker (`cx.prompt_for_paths`, the XDG portal on Linux), files dropped on the editor (`on_drop::<ExternalPaths>`, with `drag_over` tinting the pane), and paste (`Textarea::on_paste`, which sees the clipboard before the text field does and takes images and copied files).
  - **The slow part runs off the UI thread.** `cx.background_spawn` runs `papa_core::images::prepare` (decode, rotate, shrink, encode) on GPUI's thread pool; then the store saves the result on its own thread. Images are prepared one at a time, so a drop of fifty photos doesn't need fifty photos' worth of memory at once. Placeholder tiles show what's still coming.
  - **Drawing:** `img()` takes an `Arc<gpui::Image>` holding encoded bytes. GPUI decodes it on a background thread the first time it's drawn and caches the pixels, keyed by a hash of the bytes. It keeps them, and their GPU texture, until told otherwise, so the gallery and lightbox call `release` for images they stop showing. Without it, browsing photos grows the app by about 16 MB per full-size image viewed.
  - **The lightbox** is the last child of `PapaApp`'s root, so it covers the panes; gpui-kit's dialogs and notifications still draw above it, which the "Delete this image?" confirmation needs. The thumbnail is drawn under the full-size image, because GPUI waits 200 ms before showing an `img`'s loading placeholder. The full-size image has an element id: GPUI keeps a GIF's current frame in element state, so without one a GIF wouldn't animate.
  - **The strip** scrolls sideways with GPUI's `overflow_x_scroll`, which turns a mouse wheel's vertical turns into sideways scrolling, plus gpui-kit's scrollbar sharing its `ScrollHandle`.
- **Icons:** gpui-kit's default asset source embeds only the components' icons. `assets.rs` adds the extra Lucide icons the app uses with `icon_assets!` and combines both into one `AssetSource`.
- **Window chrome:** the app draws its own title bar. On Linux it asks for client-side decorations so the compositor doesn't add a second one.
- **Settings and themes** (see [ADR 0010](adr/0010-settings-file-and-themes.md)):
  - **The settings are a global.** `settings.rs` loads `settings.json` into the `AppSettings` global before the first window opens. Anything reads it with `AppSettings::get(cx)`; `AppSettings::update` changes it, clamps values into range and saves 300 ms later on the background pool. `cx.observe_global::<AppSettings>` runs after every change: `theme.rs` rebuilds the theme and `PapaApp` redraws.
  - **Themes** are gpui-kit's: a `ThemeRegistry` global of named themes and a `Theme` global for the one in use. `theme.rs` registers the bundled themes (`assets/themes/`, compiled in with `include_str!`), watches the themes folder with `ThemeRegistry::watch_dir`, and on every change of either global builds `Theme` from the settings: the chosen light and dark themes, the mode, the interface font and the text size. `Root` sizes the interface in rems of that text size, so everything scales with it.
  - **Reloads drop bundled themes.** A reload of the themes folder keeps only gpui-kit's defaults and the folder's files, so the registry observer adds the bundled themes back.
  - **"Match system"** follows the main window's appearance, observed with `cx.observe_window_appearance`.
  - **Fonts** come from the installed families, listed once. A configured font that isn't installed falls back to the default before any text is laid out: GPUI panics on a family it can't find. The reading font and line height are set on the preview's parent element and inherited; the text field sets its own line height, so the writing font and line height go on the field itself.
  - **The Settings window** is a second window with its own root view, `SettingsPanel`, built from gpui-kit's `Settings`, `SettingPage`, `SettingGroup` and `SettingItem`. Most fields are a pair of closures that read and write the global. The font pickers are searchable `Select`s, so the panel is an entity that keeps their state. A global holds the window's handle, so a second ⌘, brings it forward; closing the main window quits the app even while Settings is open.

| Shortcut (⌘ on macOS, Ctrl elsewhere) | Action |
|---|---|
| ⌘Q | Quit (saves the open note first) |
| ⌘⇧L | Toggle light/dark (and remember it) |
| ⌘, | Open Settings |
| ⌘B | Toggle sidebar |
| ⌘N | New note for the selected child or label |
| ⌘E | Switch the note between Edit and Preview |
| ⌘F | Jump to search; Esc clears it |
| ⌘⇧O | Add images to the open note (⌘⇧I is GPUI's inspector in debug builds) |
| ⌘K | Command palette: run a command, go to a child or label, open a note, assign the open note |
| ⌘1 / ⌘2 / ⌘3 | Focus the sidebar / note list / note |
| → or Enter (sidebar) | Focus the note list |
| → or Enter (note list) | Focus the note; Enter in the search box too |
| ← (note list) / Esc (note) | Back one pane |
| ↑ / ↓, Enter, Esc (palette) | Move, choose; Esc clears the search, then closes |
| ← / → / Esc (lightbox open) | Previous / next image / close |
| ↑ / ↓ (sidebar focused) | Previous / next entry |
| ↑ / ↓ (note list or search focused) | Previous / next note |
| Enter (in the title) | Move to the body |
| Enter / Esc (in a dialog) | Save / cancel |

## Files

| What | Where (Linux / macOS) | Override |
|---|---|---|
| Vault | `~/.local/share/papa/vault.db` / `~/Library/Application Support/papa/vault.db` | `PAPA_VAULT` (a file) |
| Settings | `~/.config/papa/settings.json` / `~/Library/Application Support/papa/settings.json` | `PAPA_CONFIG_DIR` (a folder) |
| Your themes | `themes/` next to `settings.json` | `PAPA_CONFIG_DIR` |

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
- **Images** are stored in the vault, one row per version: thumbnail, display copy and the untouched original. Turso reads a row's whole record even when a query asks for one column, so the bytes can't share a row with anything a list reads. An index on `image (note_id, id)` steers Turso's planner away from scanning the bytes; a test checks the plans. See [ADR 0009](adr/0009-images-stored-in-the-vault.md).
- **The write-ahead log:** Turso leaves recent commits in `vault.db-wal` after the app quits; it doesn't checkpoint on close like SQLite does. Nothing is lost, but a backup must checkpoint first or copy both files (Phase 9).

| Table | Columns | Since |
|---|---|---|
| `child` | `id` (UUIDv7 text), `name`, `color` (0xRRGGBB), `sort_order` | v1 |
| `label` | `id` (UUIDv7 text), `name` (unique, `COLLATE NOCASE`) | v1 |
| `note` | `id`, `title`, `body_md` (Markdown), `created_at`, `updated_at`, `deleted_at` (Unix ms) | v2 |
| `note_child` | `note_id`, `child_id` (both cascade on delete) | v2 |
| `note_label` | `note_id`, `label_id` (both cascade on delete) | v2 |
| `image` | `id`, `note_id`, `mime`, `width`, `height`, `sha256`, `created_at`; unique per note and hash | v3 |
| `image_data` | `image_id`, `kind` (`thumb` / `display` / `original`), `mime`, `bytes` | v3 |

## Data flow (planned)

1. The father's vault is a local Turso database. It is the source of truth and works offline.
2. When a note is shared with a child, it is encrypted on the father's machine. It is then written to a local replica of that child's share database and pushed to Turso Cloud.
3. The child's app signs in through the relay (father's email + access key). It receives a read-only token scoped to its own database, pulls, and decrypts locally.
4. Messages are append-only encrypted rows in the same share database. Both sides push and pull them.

## Threading

- **UI thread:** GPUI's executors. Long work is awaited in GPUI tasks, never run inline.
- **GPUI's background thread pool:** CPU-heavy work handed off with `cx.background_spawn`: preparing images, and GPUI's own image decoding. Saving `settings.json` runs there too.
- **`papa-store` thread:** owns the database connection and runs a single-threaded tokio runtime (tokio is also what `turso`'s sync needs later). It exits when the last `Store` handle is dropped.

## Security model (planned)

- Optional encryption at rest for the local vault, keyed from the father's password (Argon2id).
- End-to-end encryption for everything in the cloud (XChaCha20-Poly1305). The relay and Turso only see ciphertext, ids and timestamps.
- Tokens are short-lived and scoped to a single database. No admin or Platform credentials ship in the app.
