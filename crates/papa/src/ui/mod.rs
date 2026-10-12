//! Presentational pieces of the window.
//!
//! Each is a `RenderOnce` component: a short-lived struct built from data the
//! parent passes in, consumed when rendered. State lives in the `PapaApp`
//! entity; these only describe how it looks and report clicks back through
//! callbacks.

mod note_list;
mod note_view;
mod sidebar;
mod title_bar;

pub use note_list::NoteList;
pub use note_view::NoteView;
pub use sidebar::AppSidebar;
pub use title_bar::AppTitleBar;
