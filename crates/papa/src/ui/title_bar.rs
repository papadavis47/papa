use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{
    ActiveTheme as _, IconName, Sizable as _, StyledExt as _, TitleBar,
    button::{Button, ButtonVariants as _},
    h_flex,
    tag::Tag,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{App, IntoElement, ParentElement as _, RenderOnce, Styled as _, Window, div};

use super::shortcut;
use crate::app::{
    APP_CONTEXT, LockVault, SyncNow, ToggleCommandPalette, ToggleSidebar, ToggleTheme,
};

/// The custom title bar. The app's name and the theme toggle are always
/// there, and a "Demo" tag in `papa --demo`; the rest depends on the mode:
///
/// - **Father:** the sidebar toggle on the left; the command palette and
///   the lock on the right. While the vault is locked, neither shows.
/// - **Child:** "Check for new notes" on the right.
/// - **Welcome:** nothing else yet.
///
/// The buttons dispatch the same actions as the keyboard shortcuts, so both
/// paths run the same handler.
#[derive(IntoElement)]
pub struct AppTitleBar {
    mode: Mode,
}

enum Mode {
    Father {
        sidebar_visible: bool,
        locked: bool,
        /// The vault has a password, so it can be locked.
        encrypted: bool,
    },
    Child,
    Welcome,
}

impl AppTitleBar {
    pub fn new(sidebar_visible: bool, locked: bool, encrypted: bool) -> Self {
        Self {
            mode: Mode::Father {
                sidebar_visible,
                locked,
                encrypted,
            },
        }
    }

    pub fn child() -> Self {
        Self { mode: Mode::Child }
    }

    pub fn welcome() -> Self {
        Self {
            mode: Mode::Welcome,
        }
    }
}

impl RenderOnce for AppTitleBar {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let (sidebar_visible, locked, encrypted, father) = match self.mode {
            Mode::Father {
                sidebar_visible,
                locked,
                encrypted,
            } => (sidebar_visible, locked, encrypted, true),
            Mode::Child | Mode::Welcome => (false, false, false, false),
        };
        let child = matches!(self.mode, Mode::Child);
        let sidebar_icon = if sidebar_visible {
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
                        .when(father && !locked, |this| {
                            this.child(
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
                        })
                        .child(div().text_sm().font_semibold().child("Papa"))
                        .when(papa_core::paths::demo_dir().is_some(), |this| {
                            this.child(Tag::info().small().child("Demo"))
                        }),
                )
                .child(
                    h_flex()
                        .gap_1()
                        .when(father && !locked, |this| {
                            this.child(
                                Button::new("command-palette")
                                    .ghost()
                                    .small()
                                    .icon(IconName::Search)
                                    .label("Search")
                                    .children(shortcut(&ToggleCommandPalette, window))
                                    .tooltip("Find notes, children, labels and commands")
                                    .on_click(|_, window, cx| {
                                        window.dispatch_action(Box::new(ToggleCommandPalette), cx)
                                    }),
                            )
                        })
                        .when(child, |this| {
                            this.child(
                                Button::new("check-notes")
                                    .ghost()
                                    .small()
                                    .icon(gpui_kit::assets::IconName::RefreshCw)
                                    .tooltip_with_action(
                                        "Check for new notes",
                                        &SyncNow,
                                        Some(APP_CONTEXT),
                                    )
                                    .on_click(|_, window, cx| {
                                        window.dispatch_action(Box::new(SyncNow), cx)
                                    }),
                            )
                        })
                        .when(encrypted, |this| {
                            this.child(
                                Button::new("lock")
                                    .ghost()
                                    .small()
                                    .icon(Lucide::Lock)
                                    .tooltip_with_action("Lock Papa", &LockVault, Some(APP_CONTEXT))
                                    .on_click(|_, window, cx| {
                                        window.dispatch_action(Box::new(LockVault), cx)
                                    }),
                            )
                        })
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
                ),
        )
    }
}
