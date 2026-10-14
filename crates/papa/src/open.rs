//! Opening a file or folder in the app the desktop uses for it.
//!
//! GPUI's `open_with_system` runs `xdg-open` on Linux with Papa's own
//! environment. That breaks when Papa was started from a terminal installed
//! as a snap (Ghostty's, say): the snap adds variables pointing into its own
//! folder, such as `GDK_PIXBUF_MODULE_FILE`, and the PDF viewer `xdg-open`
//! starts loads the snap's libraries instead of the system's and crashes.
//! When Papa sees it is running with a snap's environment, it starts
//! `xdg-open` itself, with the snap's additions taken out.

#[cfg(any(target_os = "linux", test))]
use std::ffi::{OsStr, OsString};
use std::path::Path;
#[cfg(target_os = "linux")]
use std::process::Command;

use gpui_kit::App;

/// Opens `path` with the desktop's default app for it.
pub fn open(path: &Path, cx: &App) {
    #[cfg(target_os = "linux")]
    if let Some(snap) = std::env::var_os("SNAP") {
        let env = without_snap(std::env::vars_os(), &snap);
        let mut command = Command::new("xdg-open");
        command.arg(path).env_clear().envs(env);
        // Waits for the opener to exit, so it doesn't linger as a zombie.
        cx.background_executor()
            .spawn(async move {
                if let Err(err) = command.status() {
                    eprintln!("papa: couldn't run xdg-open: {err}");
                }
            })
            .detach();
        return;
    }
    cx.open_with_system(path);
}

/// `vars` without what the snap installed at `snap` (`/snap/ghostty/820`)
/// added: its `SNAP*` variables, and any path inside the snap. Lists of
/// paths (`XDG_DATA_DIRS`, `PATH`) keep their other entries.
///
/// Snaps are Linux-only; elsewhere this exists only for its tests.
#[cfg(any(target_os = "linux", test))]
fn without_snap(
    vars: impl IntoIterator<Item = (OsString, OsString)>,
    snap: &OsStr,
) -> Vec<(OsString, OsString)> {
    let snap = Path::new(snap);
    vars.into_iter()
        .filter(|(name, _)| !name.to_string_lossy().starts_with("SNAP"))
        .filter_map(|(name, value)| {
            let entries: Vec<_> = std::env::split_paths(&value).collect();
            let kept: Vec<_> = entries
                .iter()
                .filter(|entry| !entry.starts_with(snap))
                .collect();
            if kept.len() == entries.len() {
                // Nothing of the snap's: kept exactly as it was, so values
                // that aren't paths (`unix:path=…`) pass through untouched.
                return Some((name, value));
            }
            let joined = std::env::join_paths(kept).ok()?;
            (!joined.is_empty()).then_some((name, joined))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
        pairs
            .iter()
            .map(|(name, value)| (name.into(), value.into()))
            .collect()
    }

    #[test]
    fn the_snaps_additions_are_removed() {
        let vars = env(&[
            ("SNAP", "/snap/ghostty/820"),
            ("SNAP_NAME", "ghostty"),
            (
                "GDK_PIXBUF_MODULE_FILE",
                "/snap/ghostty/820/usr/lib/x86_64-linux-gnu/gdk-pixbuf-2.0/2.10.0/loaders.cache",
            ),
            (
                "XDG_DATA_DIRS",
                "/snap/ghostty/820/usr/share:/usr/local/share:/usr/share",
            ),
            ("PATH", "/snap/ghostty/820/bin:/usr/bin:/snap/bin"),
            ("HOME", "/home/john"),
            ("LANG", "en_US.UTF-8"),
            ("EMPTY", ""),
        ]);

        let cleaned = without_snap(vars, OsStr::new("/snap/ghostty/820"));

        assert_eq!(
            cleaned,
            env(&[
                ("XDG_DATA_DIRS", "/usr/local/share:/usr/share"),
                // Other snaps' commands stay reachable.
                ("PATH", "/usr/bin:/snap/bin"),
                ("HOME", "/home/john"),
                ("LANG", "en_US.UTF-8"),
                ("EMPTY", ""),
            ])
        );
    }

    #[test]
    fn a_sibling_folder_with_the_same_prefix_is_kept() {
        let vars = env(&[("GTK_PATH", "/snap/ghostty/8200/lib")]);
        assert_eq!(
            without_snap(vars.clone(), OsStr::new("/snap/ghostty/820")),
            vars
        );
    }
}
