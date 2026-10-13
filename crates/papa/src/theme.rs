//! Themes and fonts: the bundled themes, the father's own from the themes
//! folder, and the appearance settings applied to gpui-kit's `Theme`.
//!
//! Three globals take part. `ThemeRegistry` holds every theme by name.
//! `Theme` is the one in use, read by every component. `AppSettings` says
//! which themes, fonts and sizes the father chose. This module observes the
//! first and the last, and rebuilds `Theme` whenever either changes.

use gpui_kit::component::{ActiveTheme as _, Theme, ThemeConfig, ThemeMode, ThemeRegistry};
use gpui_kit::{App, Global, SharedString, Window, WindowAppearance, px};
use papa_core::paths;
use papa_core::settings::{Appearance, ColorMode};

use crate::settings::AppSettings;

/// The bundled theme files, compiled into the binary. See
/// `assets/themes/README.md`.
const BUNDLED: [&str; 7] = [
    include_str!("../../../assets/themes/papa-paper.json"),
    include_str!("../../../assets/themes/ayu.json"),
    include_str!("../../../assets/themes/catppuccin.json"),
    include_str!("../../../assets/themes/everforest.json"),
    include_str!("../../../assets/themes/flexoki.json"),
    include_str!("../../../assets/themes/gruvbox.json"),
    include_str!("../../../assets/themes/solarized.json"),
];

/// What `.SystemUIFont` resolved to before any setting changed it, so
/// choosing "System default" again can go back to it.
struct DefaultUiFont(SharedString);

impl Global for DefaultUiFont {}

/// The installed font families, sorted. Listing them asks the OS, so it
/// happens once, the first time a font is looked up.
struct InstalledFonts(Vec<SharedString>);

impl Global for InstalledFonts {}

/// Whether the OS is in light or dark mode, for [`ColorMode::System`].
/// Updated from the main window, which knows it more reliably on Linux than
/// the app does.
struct SystemAppearance(WindowAppearance);

impl Global for SystemAppearance {}

/// Registers the bundled themes, starts watching the themes folder and
/// applies the settings. Call once, after `settings::init`.
pub fn init(cx: &mut App) {
    cx.set_global(DefaultUiFont(cx.theme().font_family.clone()));
    let appearance = cx.window_appearance();
    cx.set_global(SystemAppearance(appearance));
    load_bundled(cx);

    // The registry changes when the themes folder does. Reloading it keeps
    // only gpui-kit's defaults and the folder's files, so the bundled themes
    // are added back. That changes the registry again, but then nothing is
    // missing and this only re-applies the settings: a theme the settings
    // name may just have appeared, and a theme file may set its own fonts.
    cx.observe_global::<ThemeRegistry>(|cx| {
        if bundled_missing(cx) {
            load_bundled(cx);
        }
        apply(cx);
    })
    .detach();
    cx.observe_global::<AppSettings>(apply).detach();

    match paths::themes_dir() {
        // Creates the folder, then loads it on the next turn of the event
        // loop. The registry observer above takes it from there.
        Ok(dir) => {
            if let Err(err) = ThemeRegistry::watch_dir(dir, cx, |_| {}) {
                eprintln!("papa: not watching the themes folder: {err:#}");
            }
        }
        Err(err) => eprintln!("papa: no themes folder: {err}"),
    }
    apply(cx);
}

fn load_bundled(cx: &mut App) {
    let registry = ThemeRegistry::global_mut(cx);
    for file in BUNDLED {
        // Skips themes already registered, such as a user theme of the same name.
        registry
            .load_themes_from_str(file)
            .expect("bundled themes are valid; see the test below");
    }
}

/// Bundled themes are named in their first theme: "Papa Paper Light" first.
fn bundled_missing(cx: &App) -> bool {
    let themes = ThemeRegistry::global(cx).themes();
    !themes.contains_key(papa_core::settings::DEFAULT_LIGHT_THEME)
        || !themes.contains_key(papa_core::settings::DEFAULT_DARK_THEME)
}

/// Builds the active theme from the appearance settings: the chosen light
/// and dark themes, the mode, the interface font and the text size.
pub fn apply(cx: &mut App) {
    let appearance = AppSettings::get(cx).appearance.clone();
    let light = theme_named(&appearance.light_theme, ThemeMode::Light, cx);
    let dark = theme_named(&appearance.dark_theme, ThemeMode::Dark, cx);
    let mode = effective_mode(&appearance, cx);
    let ui_font = ui_font(cx);

    let theme = Theme::global_mut(cx);
    theme.light_theme = light;
    theme.dark_theme = dark;
    // Loads the mode's theme, colours and all, and redraws every window.
    Theme::change(mode, None, cx);
    // After the theme file, so the settings win over fonts a theme sets.
    // `Root` sizes everything in rems of `font_size`, so the whole interface
    // scales with it.
    Theme::update(cx, |theme| {
        theme.font_family = ui_font;
        theme.font_size = px(appearance.text_size);
    });
}

/// The registered theme called `name`, if it is for `mode`; otherwise
/// gpui-kit's default for `mode`. A theme removed from the themes folder,
/// or one picked for the wrong mode in a hand-edited file, falls back.
fn theme_named(name: &str, mode: ThemeMode, cx: &App) -> std::rc::Rc<ThemeConfig> {
    let registry = ThemeRegistry::global(cx);
    match registry.themes().get(name) {
        Some(theme) if theme.mode == mode => theme.clone(),
        _ => registry.default_themes()[&mode].clone(),
    }
}

fn effective_mode(appearance: &Appearance, cx: &App) -> ThemeMode {
    match appearance.mode {
        ColorMode::Light => ThemeMode::Light,
        ColorMode::Dark => ThemeMode::Dark,
        ColorMode::System => cx.global::<SystemAppearance>().0.into(),
    }
}

/// The main window's appearance, now or after the OS switched. Only
/// matters in [`ColorMode::System`].
pub fn set_system_appearance(window: &Window, cx: &mut App) {
    cx.set_global(SystemAppearance(window.appearance()));
    if AppSettings::get(cx).appearance.mode == ColorMode::System {
        apply(cx);
    }
}

/// Flips between light and dark, and remembers the choice. Leaves
/// "match system" for an explicit mode.
pub fn toggle(cx: &mut App) {
    let next = if cx.theme().mode.is_dark() {
        ColorMode::Light
    } else {
        ColorMode::Dark
    };
    AppSettings::update(cx, |settings| settings.appearance.mode = next);
}

/// Names of the registered themes for `mode`, sorted.
pub fn theme_names(mode: ThemeMode, cx: &App) -> Vec<SharedString> {
    let mut names: Vec<SharedString> = ThemeRegistry::global(cx)
        .themes()
        .values()
        .filter(|theme| theme.mode == mode)
        .map(|theme| theme.name.clone())
        .collect();
    names.sort_by_key(|name| name.to_lowercase());
    names
}

// --- Fonts ---

/// The installed font families, sorted.
pub fn installed_fonts(cx: &mut App) -> &[SharedString] {
    if !cx.has_global::<InstalledFonts>() {
        let names = cx.text_system().all_font_names();
        let names = names.into_iter().map(SharedString::from).collect();
        cx.set_global(InstalledFonts(names));
    }
    &cx.global::<InstalledFonts>().0
}

/// `name`, if it's set and installed. GPUI panics laying out text in a
/// family it can't find, so a font from the settings file is never used
/// unchecked: it may have been uninstalled, or mistyped by hand.
fn installed(name: Option<&str>, cx: &mut App) -> Option<SharedString> {
    let name = name?;
    let fonts = installed_fonts(cx);
    fonts
        .binary_search_by(|font| font.as_ref().cmp(name))
        .ok()
        .map(|ix| fonts[ix].clone())
}

/// The interface font: the chosen one, or the system's.
pub fn ui_font(cx: &mut App) -> SharedString {
    let chosen = AppSettings::get(cx).appearance.ui_font.clone();
    installed(chosen.as_deref(), cx).unwrap_or_else(|| cx.global::<DefaultUiFont>().0.clone())
}

/// The font of a note's rendered text: the chosen one, or the interface font.
pub fn reading_font(cx: &mut App) -> SharedString {
    let chosen = AppSettings::get(cx).appearance.reading_font.clone();
    installed(chosen.as_deref(), cx).unwrap_or_else(|| cx.theme().font_family.clone())
}

/// The font of a note's Markdown while editing: the chosen one, or the
/// interface font.
pub fn writing_font(cx: &mut App) -> SharedString {
    let chosen = AppSettings::get(cx).appearance.writing_font.clone();
    installed(chosen.as_deref(), cx).unwrap_or_else(|| cx.theme().font_family.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_themes_load_and_come_in_pairs() {
        let mut all = ThemeRegistry::default();
        let mut count = 0;
        for file in BUNDLED {
            let mut registry = ThemeRegistry::default();
            registry.load_themes_from_str(file).unwrap();
            let themes = registry.themes();
            let dark = themes.values().filter(|theme| theme.mode.is_dark()).count();
            assert!(
                dark > 0 && dark < themes.len(),
                "every file has a light and a dark theme"
            );
            count += themes.len();
            all.load_themes_from_str(file).unwrap();
        }
        assert_eq!(all.themes().len(), count, "theme names are unique");
        for (name, mode) in [
            (papa_core::settings::DEFAULT_LIGHT_THEME, ThemeMode::Light),
            (papa_core::settings::DEFAULT_DARK_THEME, ThemeMode::Dark),
        ] {
            assert_eq!(all.themes()[name].mode, mode, "{name}");
        }
    }
}
