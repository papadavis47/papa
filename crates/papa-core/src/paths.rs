//! Where Papa keeps its files: the vault in the OS data folder, settings
//! and user themes in the OS config folder.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::{Error, Result};

/// Overrides the vault location. Handy for development and demos, so the
/// real vault is never touched: `PAPA_VAULT=/tmp/papa-dev.db cargo run`.
pub const VAULT_ENV: &str = "PAPA_VAULT";

/// The vault file to open: `$PAPA_VAULT` if set, otherwise `vault.db` in a
/// `papa` folder under the OS data directory (`~/.local/share` on Linux,
/// `~/Library/Application Support` on macOS).
pub fn vault_path() -> Result<PathBuf> {
    choose_vault_path(std::env::var_os(VAULT_ENV), dirs::data_dir())
}

/// Overrides the config folder, which holds `settings.json` and `themes/`.
/// Like [`VAULT_ENV`], it keeps development away from the real settings.
pub const CONFIG_ENV: &str = "PAPA_CONFIG_DIR";

/// The folder for settings and user themes: `$PAPA_CONFIG_DIR` if set,
/// otherwise `papa` under the OS config folder (`~/.config` on Linux,
/// `~/Library/Application Support` on macOS).
pub fn config_dir() -> Result<PathBuf> {
    choose_config_dir(std::env::var_os(CONFIG_ENV), dirs::config_dir())
}

/// The settings file, `settings.json` in [`config_dir`].
pub fn settings_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("settings.json"))
}

/// The folder the father can drop theme files into, `themes` in
/// [`config_dir`]. The app creates it and reloads its themes on every change.
pub fn themes_dir() -> Result<PathBuf> {
    Ok(config_dir()?.join("themes"))
}

/// Where the save dialog for an exported PDF starts: the Documents folder,
/// or the home folder if there's none.
pub fn export_dir() -> PathBuf {
    dirs::document_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_default()
}

/// `path` for display, with the home folder shortened to `~`.
pub fn display(path: &Path) -> String {
    shorten_home(path, dirs::home_dir().as_deref())
}

fn shorten_home(path: &Path, home: Option<&Path>) -> String {
    match home.and_then(|home| path.strip_prefix(home).ok()) {
        Some(rest) => Path::new("~").join(rest).display().to_string(),
        None => path.display().to_string(),
    }
}

/// The decision behind [`vault_path`], with its inputs passed in so tests
/// don't have to change the process environment.
fn choose_vault_path(env: Option<OsString>, data_dir: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(path) = env.filter(|path| !path.is_empty()) {
        return Ok(path.into());
    }
    data_dir
        .map(|dir| dir.join("papa").join("vault.db"))
        .ok_or(Error::NoDataDir)
}

fn choose_config_dir(env: Option<OsString>, config_dir: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(path) = env.filter(|path| !path.is_empty()) {
        return Ok(path.into());
    }
    config_dir
        .map(|dir| dir.join("papa"))
        .ok_or(Error::NoConfigDir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_environment_variable_wins() {
        let path = choose_vault_path(Some("/tmp/dev.db".into()), Some("/data".into()));
        assert_eq!(path.unwrap(), PathBuf::from("/tmp/dev.db"));
    }

    #[test]
    fn defaults_to_the_data_directory() {
        let path = choose_vault_path(None, Some("/data".into()));
        assert_eq!(path.unwrap(), PathBuf::from("/data/papa/vault.db"));
    }

    #[test]
    fn an_empty_variable_counts_as_unset() {
        let path = choose_vault_path(Some("".into()), Some("/data".into()));
        assert_eq!(path.unwrap(), PathBuf::from("/data/papa/vault.db"));
    }

    #[test]
    fn the_config_folder_follows_the_same_rules() {
        let dir = choose_config_dir(Some("/tmp/papa-config".into()), Some("/config".into()));
        assert_eq!(dir.unwrap(), PathBuf::from("/tmp/papa-config"));
        let dir = choose_config_dir(Some("".into()), Some("/config".into()));
        assert_eq!(dir.unwrap(), PathBuf::from("/config/papa"));
        assert!(matches!(
            choose_config_dir(None, None),
            Err(Error::NoConfigDir)
        ));
    }

    #[test]
    fn paths_under_home_are_shortened() {
        let home = Path::new("/home/dad");
        assert_eq!(
            shorten_home(
                Path::new("/home/dad/.local/share/papa/vault.db"),
                Some(home)
            ),
            "~/.local/share/papa/vault.db"
        );
        assert_eq!(
            shorten_home(Path::new("/tmp/dev.db"), Some(home)),
            "/tmp/dev.db"
        );
    }

    #[test]
    fn without_a_data_directory_asks_for_the_variable() {
        assert!(matches!(
            choose_vault_path(None, None),
            Err(Error::NoDataDir)
        ));
    }
}
