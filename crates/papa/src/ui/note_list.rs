use gpui_kit::component::{ActiveTheme as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, IntoElement, ParentElement as _, RenderOnce, SharedString, Styled as _, Window, div,
};

/// The middle pane. Until notes arrive in Phase 3 it shows the selected
/// child's name over a message about why the list is empty.
#[derive(IntoElement)]
pub struct NoteList {
    heading: Option<SharedString>,
    message: SharedString,
}

impl NoteList {
    pub fn new(heading: Option<SharedString>, message: impl Into<SharedString>) -> Self {
        Self {
            heading,
            message: message.into(),
        }
    }
}

impl RenderOnce for NoteList {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        v_flex()
            .size_full()
            .when_some(self.heading, |this, heading| {
                this.child(
                    h_flex()
                        .px_4()
                        .py_3()
                        .border_b_1()
                        .border_color(cx.theme().border)
                        .child(div().font_semibold().child(heading)),
                )
            })
            .child(
                v_flex()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .px_6()
                    .text_sm()
                    .text_center()
                    .text_color(cx.theme().muted_foreground)
                    .child(self.message),
            )
    }
}
