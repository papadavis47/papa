use std::rc::Rc;

use gpui_kit::component::{ActiveTheme as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, InteractiveElement as _, IntoElement, ParentElement as _, RenderOnce, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div,
};

use crate::placeholder::{Note, NoteId};

type OnSelect = Rc<dyn Fn(&NoteId, &mut Window, &mut App)>;

/// The middle pane: a heading and one row per note.
#[derive(IntoElement)]
pub struct NoteList {
    heading: SharedString,
    notes: Vec<Note>,
    selected: Option<NoteId>,
    on_select: OnSelect,
}

impl NoteList {
    pub fn new(
        heading: impl Into<SharedString>,
        notes: Vec<Note>,
        selected: Option<NoteId>,
        on_select: impl Fn(&NoteId, &mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            heading: heading.into(),
            notes,
            selected,
            on_select: Rc::new(on_select),
        }
    }
}

impl RenderOnce for NoteList {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let count = match self.notes.len() {
            1 => "1 note".to_string(),
            n => format!("{n} notes"),
        };

        let rows = self.notes.into_iter().map(|note| {
            let id = note.id;
            let selected = self.selected == Some(id);
            let on_select = self.on_select.clone();
            let meta = note
                .labels
                .iter()
                .map(|label| format!("#{label}"))
                .chain([note.date.to_string()])
                .collect::<Vec<_>>()
                .join(" · ");

            v_flex()
                .id(("note", id.0 as usize))
                .gap_0p5()
                .px_3()
                .py_2()
                .rounded(cx.theme().radius)
                .cursor_pointer()
                .when(selected, |this| this.bg(cx.theme().list_active))
                .when(!selected, |this| {
                    this.hover(|style| style.bg(cx.theme().list_hover))
                })
                .child(div().text_sm().font_medium().child(note.title))
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(meta),
                )
                .on_click(move |_, window, cx| on_select(&id, window, cx))
        });

        v_flex()
            .size_full()
            .child(
                h_flex()
                    .gap_2()
                    .px_4()
                    .py_3()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(div().font_semibold().child(self.heading))
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(count),
                    ),
            )
            .child(
                v_flex()
                    .id("note-rows")
                    .flex_1()
                    .min_h_0()
                    .p_2()
                    .gap_1()
                    .overflow_y_scroll()
                    .children(rows),
            )
    }
}
