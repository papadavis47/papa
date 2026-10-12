use gpui_kit::component::{
    ActiveTheme as _, IconName, Sizable as _, StyledExt as _, TitleBar,
    button::{Button, ButtonVariants as _},
    h_flex,
};
use gpui_kit::{App, IntoElement, ParentElement as _, RenderOnce, Styled as _, Window, div};

use crate::app::{APP_CONTEXT, ToggleSidebar, ToggleTheme};

/// The custom title bar: sidebar toggle and app name on the left, theme toggle
/// on the right.
///
/// The buttons dispatch the same actions as the keyboard shortcuts, so both
/// paths run the same handler.
#[derive(IntoElement)]
pub struct AppTitleBar {
    sidebar_visible: bool,
}

impl AppTitleBar {
    pub fn new(sidebar_visible: bool) -> Self {
        Self { sidebar_visible }
    }
}

impl RenderOnce for AppTitleBar {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let sidebar_icon = if self.sidebar_visible {
            IconName::PanelLeftClose
        } else {
            IconName::PanelLeftOpen
        };
        // Show what clicking will switch to.
        let theme_icon = if cx.theme().mode.is_dark() {
            IconName::Sun
        } else {
            IconName::Moon
        };

        TitleBar::new().child(
            h_flex()
                .w_full()
                .pr_2()
                .justify_between()
                .child(
                    h_flex()
                        .gap_1()
                        .child(
                            Button::new("toggle-sidebar")
                                .ghost()
                                .small()
                                .icon(sidebar_icon)
                                .tooltip_with_action(
                                    "Toggle sidebar",
                                    &ToggleSidebar,
                                    Some(APP_CONTEXT),
                                )
                                .on_click(|_, window, cx| {
                                    window.dispatch_action(Box::new(ToggleSidebar), cx)
                                }),
                        )
                        .child(div().text_sm().font_semibold().child("Papa")),
                )
                .child(
                    Button::new("toggle-theme")
                        .ghost()
                        .small()
                        .icon(theme_icon)
                        .tooltip_with_action("Toggle light/dark", &ToggleTheme, None)
                        .on_click(|_, window, cx| {
                            window.dispatch_action(Box::new(ToggleTheme), cx)
                        }),
                ),
        )
    }
}
