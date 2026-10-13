use gpui_kit::component::{ActiveTheme as _, IconName, h_flex, v_flex};
use gpui_kit::{
    Action, App, IntoElement, ParentElement as _, RenderOnce, SharedString, Styled as _, Window, px,
};

use super::empty_state::EmptyState;
use super::shortcut;
use crate::app::{
    FocusEditor, FocusNoteList, FocusSearch, FocusSidebar, NewNote, ToggleCommandPalette,
};

/// The right pane when no note is open: a reminder of the keys that get
/// somewhere from here.
#[derive(IntoElement)]
pub struct NoteView {
    /// `false` while the vault opens, or if it couldn't: nothing to do yet.
    hints: bool,
}

impl NoteView {
    pub fn new(hints: bool) -> Self {
        Self { hints }
    }
}

impl RenderOnce for NoteView {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let root = v_flex().size_full();
        if !self.hints {
            return root;
        }
        let muted = cx.theme().muted_foreground;
        let hint = |label: &'static str, actions: &[&dyn Action], window: &Window| {
            h_flex()
                .justify_between()
                .gap_4()
                .text_sm()
                .child(SharedString::from(label))
                .child(
                    h_flex().gap_1().text_color(muted).children(
                        actions
                            .iter()
                            .filter_map(|action| shortcut(*action, window)),
                    ),
                )
        };
        let hints = v_flex()
            .w(px(260.))
            .gap_2()
            .child(hint("New note", &[&NewNote], window))
            .child(hint("Find anything", &[&ToggleCommandPalette], window))
            .child(hint("Search the notes", &[&FocusSearch], window))
            .child(hint(
                "Sidebar, list, note",
                &[&FocusSidebar, &FocusNoteList, &FocusEditor],
                window,
            ));

        root.child(
            EmptyState::new("no-note", IconName::FileText, "No note open")
                .description("Pick one from the list, or start a new one.")
                .child(hints),
        )
    }
}
