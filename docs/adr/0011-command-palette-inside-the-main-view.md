# 0011. The command palette lives inside the main view and runs actions

- **Status:** Accepted
- **Date:** 2026-10-13

## Context

Phase 6 adds a ⌘K command palette. From it the father can run any command, jump to a child, label or note, and assign the open note to children and labels without the mouse. Forces at play:

- **gpui-kit has a palette component.** `Command` and `CommandState` give a search box, groups, ↑/↓/Enter/Esc and a keyboard-shortcut hint on each row. A row can carry an `Action`; choosing it dispatches the action, and the hint is the key bound to that action where the palette has focus.
- **Actions bubble along the focus path.** GPUI dispatches an action from the focused element up through its ancestors, and runs the first `on_action` handler it meets. Most of Papa's commands (`NewNote`, `TogglePreview`, `FocusSearch`, …) are handled by `PapaApp`'s root element, and their keys are bound in its key context.
- **gpui-kit's dialogs are drawn beside the app's view, not inside it.** `Root` renders the window's view and, next to it, the dialog layer. An action dispatched from inside a dialog never passes through `PapaApp`'s root. A key binding scoped to `PapaApp`'s context doesn't apply there either, so the palette couldn't show it.
- **Some commands open a dialog themselves** ("Add child…", "Delete note…"). A dialog gives focus back, on close, to whatever had it when it opened.
- **Text fields bind ⌘F** to a search of their own text. The palette's search box is a text field.

## Decision

- **The palette is an overlay inside `PapaApp`'s root element,** rendered last so it draws over the panes, like the lightbox. It has its own key context, `CommandPalette`. It is an entity, `ui::CommandPalette`, holding the `CommandState`, whether it's open, and the focus to give back.
- **What it shows is plain data.** `palette.rs` builds the groups (Actions, Go to, This note, Notes) from a snapshot of the app's state, with no GPUI types, so it is unit-tested without a window. It's rebuilt every time the palette opens. Every note is read from the vault for it, because the note list may be filtered.
- **Commands with a shortcut run as their action.** Their row carries the action: `Command` dispatches it from inside `PapaApp`'s tree, so the same handler runs as for the key, and the row shows the key. The rest emit an event, and `PapaApp` handles them once the palette has closed and given focus back. A dialog they open then returns focus to where the father was, not to the vanished palette.
- **The palette closes when focus leaves it.** A command that moves focus, such as "Search notes" or "Focus the sidebar", closes it that way. Otherwise choosing a row, Esc on an empty search, or a click outside closes it and gives focus back.
- **⌘F is also bound in `CommandPalette > Input`.** It's as specific as the text field's own binding and registered later, so it wins: ⌘F in the palette jumps to the note search, and the palette shows ⌘F next to "Search notes".

## Consequences

- One rule holds across the app: **actions are the single entry point for commands.** A key, a button and a palette row all dispatch the same action. A new command gets a palette row by adding a `Shortcut` variant; the `match` in `ui::command_palette` won't compile until it has an action and an icon.
- The palette isn't modal the way a dialog is. Keys it doesn't handle reach `PapaApp`, and a command that moves focus closes it. That is the behaviour we want, but it means the palette must stay inside the root, below every key context it relies on.
- Closing on focus-out relies on `cx.on_focus_out` and on the palette's focus handle sitting on the panel, not the backdrop. Clicking an element that tracks a focus handle focuses it, so a handle on the backdrop would take focus on every click outside.
- UI tests (`#[gpui_kit::test]`, with gpui-kit's `test-support` feature as a dev-dependency) drive the palette headlessly. That compiles GPUI a second time for tests, about a minute more in CI.
- Rows with a custom look would have to draw their own shortcut hint, and gpui-kit measures them again on every render. All rows use the default look for now.

### Considered and rejected

- **A gpui-kit `Dialog` hosting `Command`.** It has a backdrop and focus handling built in, but actions chosen in it wouldn't reach `PapaApp`'s handlers, and app-scoped shortcuts wouldn't show.
- **Closing first, then dispatching every command from the restored focus.** The rows would then need custom content just to show their shortcut. Only dialog-opening commands need the restored focus, and they have no shortcut, so they go through events instead.
