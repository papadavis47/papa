use gpui_kit::component::{ActiveTheme as _, StyledExt as _, v_flex};
use gpui_kit::{
    App, InteractiveElement as _, IntoElement, ParentElement as _, RenderOnce,
    StatefulInteractiveElement as _, Styled as _, Window, div,
};

use crate::placeholder::Note;

/// The right pane: the selected note, read-only. Markdown rendering and the
/// editor arrive in Phase 3; for now paragraphs are plain text.
#[derive(IntoElement)]
pub struct NoteView {
    note: Option<Note>,
    /// Names of the children the note is about.
    about: Vec<&'static str>,
}

impl NoteView {
    pub fn new(note: Option<Note>, about: Vec<&'static str>) -> Self {
        Self { note, about }
    }
}

impl RenderOnce for NoteView {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let Some(note) = self.note else {
            return v_flex()
                .id("note-view")
                .size_full()
                .items_center()
                .justify_center()
                .text_color(cx.theme().muted_foreground)
                .child("No note selected");
        };

        let meta = [note.date.to_string()]
            .into_iter()
            .chain(note.labels.iter().map(|label| format!("#{label}")))
            .chain([self.about.join(", ")])
            .collect::<Vec<_>>()
            .join(" · ");

        v_flex()
            .id("note-view")
            .size_full()
            .overflow_y_scroll()
            .gap_4()
            .px_8()
            .py_6()
            .child(
                v_flex()
                    .gap_1()
                    .child(div().text_2xl().font_semibold().child(note.title))
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(meta),
                    ),
            )
            .child(
                v_flex().gap_3().max_w(gpui_kit::rems(40.)).children(
                    note.body
                        .split("\n\n")
                        .map(|paragraph| div().child(paragraph)),
                ),
            )
    }
}
