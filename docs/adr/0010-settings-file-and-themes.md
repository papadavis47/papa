# 0010. Settings in a JSON file of their own, held as a GPUI global

- **Status:** Accepted
- **Date:** 2026-10-13

## Context

Phase 5 adds preferences: light, dark or system mode, a theme for each, three fonts, text size, line height, density and two editor options. More will follow (Vim mode, auto-lock, sync interval). Forces at play:

- **The window must look right before the vault opens.** From Phase 9 the vault may be locked behind a password. The unlock screen should already use the father's theme and text size, so the settings can't live inside the vault.
- **A file someone may edit by hand**, and one that must survive version changes in both directions: an older Papa reading a newer file, and a newer Papa reading an older one.
- **Settings are read everywhere.** The theme, the note list (density), the editor (fonts, line height, autosave delay) and the Settings window itself all need them. Passing them down through every constructor would touch most of the app.
- **gpui-kit's settings components** (`Settings`, `SettingPage`, `SettingGroup`, `SettingItem`) take each field as a pair of closures over `&App`: one reads the value, one writes it. They expect the value to be reachable from the app context.
- **Themes:** gpui-kit keeps themes in a `ThemeRegistry` global and the active one in a `Theme` global. Its `ThemeRegistry::watch_dir` reloads a folder of theme files on every change, but each reload keeps only gpui-kit's two defaults and that folder's files: themes loaded any other way are dropped.
- **Fonts:** GPUI panics laying out text in a font family that isn't installed. A font named in a hand-edited file, or uninstalled since, must not crash the app.

## Decision

- **`settings.json` in the OS config folder** (`~/.config/papa` on Linux, `~/Library/Application Support/papa` on macOS, or `$PAPA_CONFIG_DIR`), apart from the vault. Theme files the father adds go in `themes/` next to it.
- **A plain serde struct in `papa-core`** (`papa_core::settings::Settings`):
  - `#[serde(default)]` on every struct, so a missing key gets its default. Unknown keys are ignored.
  - Values out of range are clamped on load and on every change (`Settings::normalized`) rather than rejected.
  - A file that can't be parsed gives the defaults and a warning naming the line. It is left alone until the father changes a setting, which replaces it.
  - Saving writes a temporary file and renames it over the old one, so a crash can't leave half a file.
- **One GPUI global, `AppSettings`,** loaded synchronously before the first window opens. `AppSettings::update` changes it, normalizes and, if anything changed, schedules a save 300 ms later on the background pool, replacing any save already waiting. A save still waiting at quit is written synchronously.
- **Observers, not plumbing:** `theme.rs` observes `AppSettings` (and `ThemeRegistry`) and rebuilds `Theme`; views read settings while rendering, and `PapaApp` observes the global to redraw.
- **The settings own the fonts and text size.** Each change re-applies the theme, then sets the font and size, so a theme file's own fonts never win. `Root` sizes the whole interface in rems of the theme's font size, so the text size scales everything.
- **Bundled themes are compiled in** with `include_str!`: Papa Paper Light and Dark (the defaults) and six pairs from gpui-kit v0.7.1 (`assets/themes/`). When a folder reload drops them, the `ThemeRegistry` observer adds them back. A file in the themes folder with a bundled theme's name replaces that theme.
- **Fonts are chosen from the installed families**, listed once from the OS. A configured font that isn't installed is replaced by the default before any text is laid out; the file keeps the name, so reinstalling the font brings it back.
- **Settings get a window of their own** (⌘, or the sidebar entry). A second ⌘, brings it forward. Closing the main window quits the app even if Settings is open.

**Considered and rejected:**
- *Settings in the vault:* unavailable while the vault is locked or failed to open.
- *TOML:* friendlier to edit, but one more dependency, and gpui-kit's theme files are JSON already.
- *Settings in place of the panes, inside the main window:* simpler, but changes can't be judged against the note they affect.
- *Watching `settings.json` for hand edits while the app runs:* hand edits are rare and take effect at the next launch.

## Consequences

- Any view can read a setting with `AppSettings::get(cx)` and any code can change one with `AppSettings::update`. New settings are a field with a default, plus a `SettingItem`.
- The `settings.json` format is now a compatibility surface: renaming a key or an enum value needs a migration or a serde alias.
- Every settings change rebuilds the theme and redraws every window. That's cheap at this size, but a setting changed many times a second (a slider dragged) might need its own path.
- Bundled fonts (PLAN.md §3, `assets/fonts/`) are left for later: installed fonts cover the settings, and bundling adds about a megabyte per family. When they arrive, `cx.text_system().add_fonts` makes them installed families like any other.
- The font list is read once per run, so a font installed while Papa runs appears after a restart.
