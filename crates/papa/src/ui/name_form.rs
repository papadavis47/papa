use gpui_kit::component::{
    ActiveTheme as _, h_flex,
    input::{Input, InputState},
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, rgb,
};

/// Colours offered for a child, as `0xRRGGBB`.
pub const CHILD_COLORS: [u32; 8] = [
    0xe0795b, 0x5b9be0, 0x6bbf7a, 0xd4a72c, 0xa77bd6, 0xe07bb0, 0x4fb3b3, 0x8c8c8c,
];

/// The colour to suggest for a new child: the first one in [`CHILD_COLORS`]
/// no child uses yet, or, once all are taken, the next in the cycle.
pub fn suggested_color(used: &[u32]) -> u32 {
    CHILD_COLORS
        .into_iter()
        .find(|color| !used.contains(color))
        .unwrap_or(CHILD_COLORS[used.len() % CHILD_COLORS.len()])
}

/// The body of the add/edit dialogs: a name field, a line for a validation
/// error and, for children, a row of colour swatches.
///
/// An entity rather than a `RenderOnce` component because it has state that
/// lives as long as the dialog: the text typed so far and the chosen colour.
pub struct NameForm {
    name: Entity<InputState>,
    /// `Some` shows the swatches (children); `None` hides them (labels).
    color: Option<u32>,
    error: Option<SharedString>,
}

impl NameForm {
    pub fn new(
        name: &str,
        color: Option<u32>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let name = name.to_owned();
        Self {
            name: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("Name")
                    .default_value(name)
            }),
            color,
            error: None,
        }
    }

    pub fn name_input(&self) -> Entity<InputState> {
        self.name.clone()
    }

    pub fn values(&self, cx: &gpui_kit::App) -> (SharedString, Option<u32>) {
        (self.name.read(cx).value(), self.color)
    }

    pub fn set_error(&mut self, error: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.error = Some(error.into());
        cx.notify();
    }
}

impl Render for NameForm {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .gap_3()
            .child(Input::new(&self.name))
            .when_some(self.color, |this, selected| {
                this.child(h_flex().gap_2().children(CHILD_COLORS.map(|color| {
                    div()
                        .id(("swatch", color as usize))
                        .size_6()
                        .rounded_full()
                        .cursor_pointer()
                        .bg(rgb(color))
                        .border_2()
                        .border_color(if color == selected {
                            cx.theme().foreground
                        } else {
                            cx.theme().transparent
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.color = Some(color);
                            cx.notify();
                        }))
                })))
            })
            .when_some(self.error.clone(), |this, error| {
                this.child(div().text_sm().text_color(cx.theme().danger).child(error))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggests_the_first_unused_color() {
        assert_eq!(suggested_color(&[]), CHILD_COLORS[0]);
        assert_eq!(
            suggested_color(&[CHILD_COLORS[0], CHILD_COLORS[2]]),
            CHILD_COLORS[1]
        );
    }

    #[test]
    fn cycles_once_every_color_is_used() {
        let mut used = CHILD_COLORS.to_vec();
        used.push(CHILD_COLORS[0]);
        assert_eq!(suggested_color(&used), CHILD_COLORS[1]);
    }
}
