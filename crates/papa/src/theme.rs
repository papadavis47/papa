//! Light/Dark switching. Phase 5 adds the theme registry, bundled and user
//! themes, and persisting the choice in settings.json.

use gpui_kit::component::{ActiveTheme as _, Theme, ThemeMode};
use gpui_kit::{App, Window};

/// Matches the window's light or dark appearance at startup.
pub fn follow_system(window: &mut Window, cx: &mut App) {
    Theme::sync_system_appearance(Some(window), cx);
}

/// Flips between light and dark. The theme is a global, so this refreshes
/// every open window.
pub fn toggle(cx: &mut App) {
    let next = if cx.theme().mode.is_dark() {
        ThemeMode::Light
    } else {
        ThemeMode::Dark
    };
    Theme::change(next, None, cx);
}
