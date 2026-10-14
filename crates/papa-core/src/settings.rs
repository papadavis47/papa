//! The father's preferences: theme, fonts, text size, editor behaviour,
//! PDF export, auto-lock.
//!
//! They live in `settings.json` in the config folder ([`paths::settings_path`]),
//! apart from the vault. The window can then look right before the vault
//! opens, and (from Phase 9) before it is unlocked.
//!
//! The file is meant to be readable and editable by hand:
//!
//! - **Missing keys get their default** (`#[serde(default)]`), so a file
//!   written by an older Papa still loads after a field is added.
//! - **Unknown keys are ignored**, so a file written by a newer Papa still
//!   loads in an older one.
//! - **Values out of range are clamped** by [`Settings::normalized`] rather
//!   than rejected: a text size of 300 becomes the largest allowed.
//!
//! [`paths::settings_path`]: crate::paths::settings_path

use std::io::ErrorKind;
use std::ops::RangeInclusive;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

/// The light and dark themes Papa starts with. Both are bundled with the
/// app; see `assets/themes/papa-paper.json`.
pub const DEFAULT_LIGHT_THEME: &str = "Papa Paper Light";
pub const DEFAULT_DARK_THEME: &str = "Papa Paper Dark";

/// Base text size in pixels. Every size in the UI is relative to it.
pub const TEXT_SIZES: RangeInclusive<f32> = 12.0..=22.0;
pub const DEFAULT_TEXT_SIZE: f32 = 16.0;

/// Line height of a note's text, as a multiple of the text size.
pub const LINE_HEIGHTS: RangeInclusive<f32> = 1.2..=2.0;
pub const DEFAULT_LINE_HEIGHT: f32 = 1.6;

/// How long typing must pause before a note is saved, in milliseconds.
pub const AUTOSAVE_DELAYS: RangeInclusive<u64> = 200..=5000;
pub const DEFAULT_AUTOSAVE_DELAY: u64 = 500;

/// How long Papa may sit idle before it locks a vault that has a password,
/// in minutes. 0 is never.
pub const AUTO_LOCK_MINUTES: RangeInclusive<u32> = 0..=24 * 60;
pub const DEFAULT_AUTO_LOCK_MINUTES: u32 = 15;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub appearance: Appearance,
    pub editor: Editor,
    pub export: Export,
    pub privacy: Privacy,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Appearance {
    pub mode: ColorMode,
    /// The theme used in light mode, by name.
    pub light_theme: String,
    /// The theme used in dark mode, by name.
    pub dark_theme: String,
    /// Font of the interface. `None`: the system's.
    pub ui_font: Option<String>,
    /// Font of a note's rendered text (the preview). `None`: the interface font.
    pub reading_font: Option<String>,
    /// Font of a note's Markdown while editing. `None`: the interface font.
    pub writing_font: Option<String>,
    /// In pixels; see [`TEXT_SIZES`].
    pub text_size: f32,
    /// See [`LINE_HEIGHTS`].
    pub line_height: f32,
    pub density: Density,
    /// Show things at once instead of animating them in.
    pub reduce_motion: bool,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            mode: ColorMode::default(),
            light_theme: DEFAULT_LIGHT_THEME.to_owned(),
            dark_theme: DEFAULT_DARK_THEME.to_owned(),
            ui_font: None,
            reading_font: None,
            writing_font: None,
            text_size: DEFAULT_TEXT_SIZE,
            line_height: DEFAULT_LINE_HEIGHT,
            density: Density::default(),
            reduce_motion: false,
        }
    }
}

/// Light, dark, or whichever the operating system uses.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ColorMode {
    Light,
    Dark,
    #[default]
    System,
}

/// How much room the lists and panes leave around things.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Density {
    #[default]
    Comfortable,
    /// Tighter spacing, and one line per note in the note list.
    Compact,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Editor {
    /// How a note shows when it is opened. A new, empty note always opens
    /// for editing.
    pub open_notes_in: NoteMode,
    /// See [`AUTOSAVE_DELAYS`].
    pub autosave_delay_ms: u64,
    /// Edit notes with Vim's keys: Normal, Insert and Visual modes. Off by
    /// default.
    pub vim_mode: bool,
}

impl Default for Editor {
    fn default() -> Self {
        Self {
            open_notes_in: NoteMode::default(),
            autosave_delay_ms: DEFAULT_AUTOSAVE_DELAY,
            vim_mode: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NoteMode {
    #[default]
    Preview,
    Edit,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Export {
    /// The paper exported PDFs are laid out for.
    pub paper: PaperSize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PaperSize {
    /// 210 × 297 mm, the international standard.
    #[default]
    A4,
    /// 8.5 × 11 in, used in North America.
    Letter,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Privacy {
    /// See [`AUTO_LOCK_MINUTES`]. Only a vault with a password locks.
    pub auto_lock_minutes: u32,
}

impl Default for Privacy {
    fn default() -> Self {
        Self {
            auto_lock_minutes: DEFAULT_AUTO_LOCK_MINUTES,
        }
    }
}

impl Settings {
    /// Reads the settings file. A missing file isn't an error: it gives the
    /// defaults, as on first launch.
    pub fn load(path: &Path) -> Result<Self> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(err) if err.kind() == ErrorKind::NotFound => return Ok(Self::default()),
            Err(reason) => {
                return Err(Error::ReadFile {
                    path: path.to_owned(),
                    reason,
                });
            }
        };
        Self::from_json(&text).map_err(|reason| Error::InvalidSettings {
            path: path.to_owned(),
            reason,
        })
    }

    /// Parses settings, normalized.
    pub fn from_json(text: &str) -> serde_json::Result<Self> {
        serde_json::from_str::<Self>(text).map(Self::normalized)
    }

    /// The settings as pretty-printed JSON, ending in a newline.
    pub fn to_json(&self) -> String {
        let mut json = serde_json::to_string_pretty(self)
            .expect("settings are plain data and always serialize");
        json.push('\n');
        json
    }

    /// Writes the settings file, creating its folder if needed. A crash
    /// halfway through leaves the old file or the new one, never half of
    /// each (see [`write_atomic`](crate::files::write_atomic)).
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|source| Error::CreateDir {
                path: dir.to_owned(),
                source,
            })?;
        }
        crate::files::write_atomic(path, self.to_json().as_bytes())
    }

    /// The same settings with every value in range: numbers clamped, blank
    /// font and theme names replaced by their defaults.
    pub fn normalized(mut self) -> Self {
        let appearance = &mut self.appearance;
        appearance.text_size = clamp(appearance.text_size, TEXT_SIZES, DEFAULT_TEXT_SIZE);
        appearance.line_height = clamp(appearance.line_height, LINE_HEIGHTS, DEFAULT_LINE_HEIGHT);
        for font in [
            &mut appearance.ui_font,
            &mut appearance.reading_font,
            &mut appearance.writing_font,
        ] {
            *font = font.take().filter(|name| !name.trim().is_empty());
        }
        for (theme, default) in [
            (&mut appearance.light_theme, DEFAULT_LIGHT_THEME),
            (&mut appearance.dark_theme, DEFAULT_DARK_THEME),
        ] {
            if theme.trim().is_empty() {
                *theme = default.to_owned();
            }
        }
        let delay = &mut self.editor.autosave_delay_ms;
        *delay = (*delay).clamp(*AUTOSAVE_DELAYS.start(), *AUTOSAVE_DELAYS.end());
        let auto_lock = &mut self.privacy.auto_lock_minutes;
        *auto_lock = (*auto_lock).min(*AUTO_LOCK_MINUTES.end());
        self
    }
}

/// `value` within `range`; `default` if it isn't a number at all.
fn clamp(value: f32, range: RangeInclusive<f32>, default: f32) -> f32 {
    if value.is_nan() {
        default
    } else {
        value.clamp(*range.start(), *range.end())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_file_gives_the_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let settings = Settings::load(&dir.path().join("settings.json")).unwrap();
        assert_eq!(settings, Settings::default());
    }

    #[test]
    fn saved_settings_load_back() {
        let dir = tempfile::tempdir().unwrap();
        // In a folder that doesn't exist yet, as on first launch.
        let path = dir.path().join("papa").join("settings.json");
        let mut settings = Settings::default();
        settings.appearance.mode = ColorMode::Dark;
        settings.appearance.ui_font = Some("Inter".into());
        settings.appearance.density = Density::Compact;
        settings.appearance.reduce_motion = true;
        settings.editor.open_notes_in = NoteMode::Edit;
        settings.editor.vim_mode = true;
        settings.export.paper = PaperSize::Letter;
        settings.privacy.auto_lock_minutes = 0;

        settings.save(&path).unwrap();

        assert_eq!(Settings::load(&path).unwrap(), settings);
        let files: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(files, ["settings.json"], "no temporary file left behind");
    }

    #[test]
    fn saving_replaces_the_old_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, "{ \"appearance\": { \"mode\": \"dark\" } }").unwrap();

        Settings::default().save(&path).unwrap();

        assert_eq!(Settings::load(&path).unwrap(), Settings::default());
    }

    #[test]
    fn the_file_is_readable_json() {
        let json = Settings::default().to_json();
        assert!(json.contains("\"mode\": \"system\""), "{json}");
        assert!(
            json.contains("\"light_theme\": \"Papa Paper Light\""),
            "{json}"
        );
        assert!(json.ends_with("}\n"));
    }

    #[test]
    fn missing_keys_get_their_defaults() {
        let settings = Settings::from_json(r#"{ "appearance": { "mode": "light" } }"#).unwrap();

        assert_eq!(settings.appearance.mode, ColorMode::Light);
        assert_eq!(settings.appearance.text_size, DEFAULT_TEXT_SIZE);
        assert_eq!(settings.appearance.light_theme, DEFAULT_LIGHT_THEME);
        assert_eq!(settings.editor, Editor::default());
        assert_eq!(Settings::from_json("{}").unwrap(), Settings::default());
    }

    #[test]
    fn unknown_keys_are_ignored() {
        let settings = Settings::from_json(
            r#"{ "vim": true, "editor": { "autosave_delay_ms": 800, "spellcheck": true } }"#,
        )
        .unwrap();
        assert_eq!(settings.editor.autosave_delay_ms, 800);
    }

    #[test]
    fn values_out_of_range_are_clamped() {
        let settings = Settings::from_json(
            r#"{
                "appearance": { "text_size": 300, "line_height": 0.5, "ui_font": "  ", "dark_theme": "" },
                "editor": { "autosave_delay_ms": 0 },
                "privacy": { "auto_lock_minutes": 100000 }
            }"#,
        )
        .unwrap();

        assert_eq!(settings.appearance.text_size, *TEXT_SIZES.end());
        assert_eq!(settings.appearance.line_height, *LINE_HEIGHTS.start());
        assert_eq!(settings.appearance.ui_font, None);
        assert_eq!(settings.appearance.dark_theme, DEFAULT_DARK_THEME);
        assert_eq!(settings.editor.autosave_delay_ms, *AUTOSAVE_DELAYS.start());
        assert_eq!(settings.privacy.auto_lock_minutes, *AUTO_LOCK_MINUTES.end());
    }

    #[test]
    fn not_a_number_falls_back_to_the_default() {
        let mut settings = Settings::default();
        settings.appearance.text_size = f32::NAN;
        assert_eq!(
            settings.normalized().appearance.text_size,
            DEFAULT_TEXT_SIZE
        );
    }

    #[test]
    fn invalid_json_says_where() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, "{\n  \"appearance\": { \"mode\": \"sepia\" }\n}").unwrap();

        let err = Settings::load(&path).unwrap_err();

        assert!(matches!(err, Error::InvalidSettings { .. }));
        let message = err.to_string();
        assert!(message.contains("settings.json"), "{message}");
        assert!(message.contains("line 2"), "{message}");
    }
}
