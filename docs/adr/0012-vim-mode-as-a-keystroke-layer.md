# 0012. Vim mode is a keystroke layer over the text field, with a pure state machine

- **Status:** Accepted
- **Date:** 2026-10-13

## Context

Phase 7 adds an optional Vim mode to the note editor: Normal, Insert and Visual modes, the motions `h j k l w b e 0 ^ $ gg G`, the edits `x dd yy p P u Ctrl-R` and `d`/`c`/`y` with a motion, `:w` and `:q`, a mode indicator, and `j`/`k` in the lists. Forces at play:

- **gpui-kit's text field has no Vim support** and no hook for one. It does have a public API for the rest: read the text and the selection, set the selection, replace the selection (as one undoable step), and the `Undo`/`Redo` actions.
- **The field types every key it's given.** Plain letters have no key bindings in its `Input` context, so the platform turns them into text. In Normal mode `x` must delete, not type.
- **Key bindings run before an element's key listeners.** The field binds Enter, Backspace, the arrows and Esc itself, so a key-down listener around it sees those keys too late.
- **GPUI has keystroke interceptors.** `cx.intercept_keystrokes` runs a callback for every keystroke in the app, before key bindings and text input. Calling `cx.stop_propagation()` there consumes the keystroke.
- **The field draws only a thin caret.** It has no block cursor. It reports where a range is on screen (`range_to_bounds`), from the layout it drew.
- **Vim's behaviour is fiddly.** `dw` on a line's last word, `cw` acting like `ce`, linewise `dd` on the last line, and `j` remembering a column are easy to get subtly wrong. Each needs a test.
- **Off by default, at no cost.** Most fathers won't use it, and it mustn't change the editor for them.

## Decision

- **A pure state machine does the work** (`papa::vim`, no GPUI). `Vim::handle(key, text, selection)` returns `Effect`s: replace a range, select a range, undo, redo, save, leave. The parts are:
  - `text`: the note as characters and lines, with byte offsets for the field.
  - `motion`: each motion as a function of the text and the cursor.
  - `parser`: keys in, commands out. It handles counts, operators and their motions (`2d3w` deletes six words), `dd`/`cc`/`yy` and `gg`.
  - `machine`: the `Mode` enum (`Normal`, `Insert`, `Visual { anchor, head }`, `CommandLine(String)`), the register, and what each command does.

  Tests type keys into a miniature text field (a `String` that plays the effects) and compare the result, cursor included: `"one |two\nthree"` + `dw` → `"one| \nthree"`.
- **`vim::field` is the only GPUI part.** It turns a `Keystroke` into a `Key` and plays the effects on the `TextareaState`. Edits use `set_selected_range` + `replace`, so each Vim command is one step of the field's undo history, and the field still emits its change event (autosave works unchanged). `u` and Ctrl-R dispatch the field's own `Undo`/`Redo` actions.
- **Keystrokes are caught with an interceptor.** While Vim is on, `NoteEditor` holds a `VimLayer`, which owns the interceptor's `Subscription`. The callback only acts while the note's text has focus and is in Edit mode. Elsewhere (the title, the search box, other windows) keys pass through untouched. In Normal and Visual mode every key without ⌘/Ctrl is Vim's; keys Vim doesn't know are dropped, not typed. ⌘ and Ctrl shortcuts (⌘K, ⌘E, ⌘V) pass through, except Ctrl-R and Ctrl-[. In Insert mode only Esc is Vim's.
- **The block cursor is painted, not selected.** In Normal mode the field keeps a plain caret in front of the cursor's character. A `canvas` drawn after the field paints a translucent block over that character, using `range_to_bounds` from the layout the field drew in the same frame. Visual mode uses the field's real selection.
- **Off means detached.** Turning the setting off drops the `VimLayer`, and with it the interceptor. With the setting off, no Vim code runs on a keystroke.
- **`:q` saves and leaves.** Papa saves as you type, so `:q` never has unsaved work to refuse over. It saves and dispatches `FocusNoteList`, as Esc does without Vim. `:w` saves now instead of when the autosave timer fires.
- **Esc in Normal mode stays in the note.** Without Vim, Esc leaves the editor. With Vim, people press Esc out of habit to make sure they're in Normal mode, so it only cancels a half-typed command. `:q` or ⌘2 leaves.
- **The lists take Vim keys through ordinary key bindings.** While the setting is on, the sidebar's and the note list's key contexts gain a `vim` identifier. `j`/`k` move, `h`/`l` change panes and `/` searches, all bound in `Sidebar && vim && !Input` or `NoteList && vim && !Input`. `!Input` keeps the search box, which sits inside the list's pane, typing those letters.

## Consequences

- Almost all of Vim mode is tested as plain Rust, a few dozen tests that run in milliseconds. A few headless UI tests drive a real text field through the interceptor: Normal-mode keys don't type, Insert mode does, `u` uses the field's history, and `:wq` reaches the editor. They check the wiring, not the semantics.
- Each keystroke copies the note's text into a `String` and a character table. That is O(note length): microseconds for a journal note. It would need a rope-aware `Text` for a book-length document.
- Undo granularity is the field's. A Vim command is one step. An Insert session may be several steps, because the field groups typing its own way, so `u` can undo part of what was typed.
- The register is Papa's own, not the system clipboard, as in Vim without `clipboard=unnamed`. ⌘C/⌘V still use the clipboard.
- Mouse selections don't enter Visual mode. A click moves the caret, and the next motion moves it back onto a character.
- `.` (repeat), text objects (`diw`), `f`/`t`, search with `/` inside the note, and Visual-line mode are not built. The parser and the `Effect` set leave room for them.
- The block cursor relies on `range_to_bounds` and on the field painting before the canvas. A gpui-kit upgrade that changes either shows up as a missing or misplaced block, not as wrong edits.

### Considered and rejected

- **Key bindings for every Vim key** (Zed's approach), in a `vim_mode == normal` context. That needs a binding per key and mode, plus a way to stop unbound letters from typing, and the parser would be spread across the keymap.
- **`set_readonly` in Normal mode**, to stop typing. The field then refuses undo and redo, which `u` needs.
- **The block as a one-character selection.** It works without painting, but the field draws its caret at the selection's end, so the cursor looked like it sat *between* two characters. ⌘V would also have replaced the character instead of pasting at the cursor.
