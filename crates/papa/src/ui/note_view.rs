use gpui_kit::component::{ActiveTheme as _, v_flex};
use gpui_kit::{App, IntoElement, ParentElement as _, RenderOnce, Styled as _, Window};

/// The right pane. The Markdown viewer and editor arrive in Phase 3; until
/// then there is never a note to show.
#[derive(IntoElement)]
pub struct NoteView;

impl RenderOnce for NoteView {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .text_color(cx.theme().muted_foreground)
            .child("No note selected")
    }
}
