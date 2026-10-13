//! Presentational pieces of the window.
//!
//! Most are `RenderOnce` components: short-lived structs built from data the
//! parent passes in, consumed when rendered. State lives in the `PapaApp`
//! entity; these only describe how it looks and report clicks back through
//! callbacks. The exceptions are entities with state of their own:
//! `NameForm` holds a dialog's half-typed input, `NoteEditor` the open
//! note's text and its pending save, `Gallery` the note's images and
//! `Lightbox` the one shown full size.

mod command_palette;
mod dates;
pub mod dialogs;
mod empty_state;
mod gallery;
pub mod lightbox;
mod name_form;
mod note_editor;
mod note_list;
mod note_view;
mod sidebar;
mod title_bar;

pub use command_palette::{COMMAND_PALETTE_CONTEXT, CommandPalette, CommandPaletteEvent};
pub use name_form::suggested_color;
pub use note_editor::{NoteEditor, NoteEditorEvent};
pub use note_list::{EmptyNotes, NOTE_SEARCH_CONTEXT, NoteList, NoteListEvent};
pub use note_view::NoteView;
pub use sidebar::{AppSidebar, SidebarEvent};
pub use title_bar::AppTitleBar;

use gpui_kit::component::kbd::Kbd;

/// Shows an error in the active window, or any window, for code that runs
/// outside one (a global's background task, say).
pub fn notify_error(title: &'static str, error: impl std::fmt::Display, cx: &mut gpui_kit::App) {
    use gpui_kit::component::{WindowExt as _, notification::Notification};

    let Some(window) = cx.active_window().or_else(|| cx.windows().first().copied()) else {
        eprintln!("papa: {title}: {error}");
        return;
    };
    let message = sentence(error);
    window
        .update(cx, |_, window, cx| {
            window.push_notification(Notification::error(message).title(title), cx)
        })
        .ok();
}

/// The key bound to `action` in the main window, drawn as a key cap.
///
/// Looked up in the main window's key context, not where focus is: a text
/// field binds ⌘F to its own search, which would hide the app's ⌘F.
pub fn shortcut(action: &dyn gpui_kit::Action, window: &gpui_kit::Window) -> Option<Kbd> {
    Kbd::binding_for_action(action, Some(crate::app::APP_CONTEXT), window)
}

/// Error messages are lowercase by Rust convention; the UI shows sentences.
pub fn sentence(message: impl std::fmt::Display) -> String {
    let message = message.to_string();
    let mut chars = message.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => message,
    }
}
