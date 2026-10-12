//! Presentational pieces of the window.
//!
//! Most are `RenderOnce` components: short-lived structs built from data the
//! parent passes in, consumed when rendered. State lives in the `PapaApp`
//! entity; these only describe how it looks and report clicks back through
//! callbacks. The exception is `NameForm`, an entity that holds a dialog's
//! half-typed input.

pub mod dialogs;
mod name_form;
mod note_list;
mod note_view;
mod sidebar;
mod title_bar;

pub use name_form::suggested_color;
pub use note_list::NoteList;
pub use note_view::NoteView;
pub use sidebar::{AppSidebar, SidebarEvent};
pub use title_bar::AppTitleBar;

/// Error messages are lowercase by Rust convention; the UI shows sentences.
pub fn sentence(message: impl std::fmt::Display) -> String {
    let message = message.to_string();
    let mut chars = message.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => message,
    }
}
