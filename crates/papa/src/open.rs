//! Opening a file or folder in the app the desktop uses for it.
//!
//! GPUI's `open_with_system` runs `xdg-open` on Linux with Papa's own
//! environment. That breaks when Papa was started from a terminal installed
//! as a snap (Ghostty's, say): the snap adds variables pointing into its own
//! folder, such as `GDK_PIXBUF_MODULE_FILE`, and the PDF viewer `xdg-open`
//! starts loads the snap's libraries instead of the system's and crashes.
//!
//! When Papa's environment holds paths inside a snap, it starts `xdg-open`
//! itself, with those taken out. It looks for the paths, not for the `SNAP`
//! variable: a terminal may drop `SNAP` before starting the shell and still
//! pass on the rest, and paths can go through `/snap/<name>/current`
//! rather than the revision `SNAP` names.

#[cfg(any(target_os = "linux", test))]
use std::ffi::OsString;
#[cfg(any(target_os = "linux", test))]
use std::path::Component;
use std::path::Path;
#[cfg(target_os = "linux")]
use std::process::Command;

use gpui_kit::App;

/// Opens `path` with the desktop's default app for it.
pub fn open(path: &Path, cx: &App) {
    #[cfg(target_os = "linux")]
    if let Some(env) = without_snaps(std::env::vars_os()) {
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

/// `vars` without what snaps added: `SNAP` and the `SNAP_*` variables, and any path
/// inside a snap (`/snap/<name>/…`, but not `/snap/bin`, where every snap's
/// commands live). Lists of paths (`XDG_DATA_DIRS`, `PATH`) keep their other
/// entries. `None` when there was nothing to take out.
///
/// Snaps are Linux-only; elsewhere this exists only for its tests.
#[cfg(any(target_os = "linux", test))]
fn without_snaps(
    vars: impl IntoIterator<Item = (OsString, OsString)>,
) -> Option<Vec<(OsString, OsString)>> {
    let mut changed = false;
    let mut cleaned = Vec::new();
    for (name, value) in vars {
        let snap_variable = {
            let name = name.to_string_lossy();
            name == "SNAP" || name.starts_with("SNAP_")
        };
        if snap_variable {
            changed = true;
            continue;
        }
        let entries: Vec<_> = std::env::split_paths(&value).collect();
        let kept: Vec<_> = entries.iter().filter(|entry| !in_snap(entry)).collect();
        if kept.len() == entries.len() {
            // Nothing of a snap's: kept exactly as it was, so values that
            // aren't paths (`unix:path=…`) pass through untouched.
            cleaned.push((name, value));
            continue;
        }
        changed = true;
        if let Ok(joined) = std::env::join_paths(kept)
            && !joined.is_empty()
        {
            cleaned.push((name, joined));
        }
    }
    changed.then_some(cleaned)
}

/// Whether `path` is inside an installed snap: `/snap/<name>/<anything>`.
#[cfg(any(target_os = "linux", test))]
fn in_snap(path: &Path) -> bool {
    let mut parts = path.components();
    matches!(
        (parts.next(), parts.next(), parts.next(), parts.next()),
        (
            Some(Component::RootDir),
            Some(Component::Normal(snap)),
            Some(Component::Normal(name)),
            Some(Component::Normal(_)),
        ) if snap == "snap" && name != "bin"
    )
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

        let cleaned = without_snaps(vars);

        assert_eq!(
            cleaned.as_deref(),
            Some(
                &env(&[
                    ("XDG_DATA_DIRS", "/usr/local/share:/usr/share"),
                    // Other snaps' commands stay reachable.
                    ("PATH", "/usr/bin:/snap/bin"),
                    ("HOME", "/home/john"),
                    ("LANG", "en_US.UTF-8"),
                    ("EMPTY", ""),
                ])[..]
            )
        );
    }

    /// What crashed the PDF viewer on Pop!_OS: the terminal had dropped
    /// `SNAP`, but not the snap's pixbuf loaders.
    #[test]
    fn snap_paths_are_removed_without_the_snap_variable() {
        let vars = env(&[
            (
                "GDK_PIXBUF_MODULE_FILE",
                "/snap/ghostty/820/usr/lib/x86_64-linux-gnu/gdk-pixbuf-2.0/2.10.0/loaders.cache",
            ),
            ("GTK_PATH", "/snap/ghostty/current/usr/lib/gtk-3.0"),
            ("XDG_DATA_DIRS", "/usr/share:/snap/ghostty/820/usr/share"),
            ("HOME", "/home/john"),
        ]);

        assert_eq!(
            without_snaps(vars).as_deref(),
            Some(&env(&[("XDG_DATA_DIRS", "/usr/share"), ("HOME", "/home/john")])[..])
        );
    }

    #[test]
    fn an_environment_without_snaps_is_left_alone() {
        let vars = env(&[
            ("PATH", "/usr/bin:/snap/bin"),
            // Not a snap's, despite the name.
            ("SNAPSHOT_DIR", "/home/john/backups"),
            ("HOME", "/home/john/snap/ghostty"),
            ("DBUS_SESSION_BUS_ADDRESS", "unix:path=/run/user/1000/bus"),
        ]);
        assert_eq!(without_snaps(vars), None);
        assert!(!in_snap(Path::new("/snap/bin/firefox")));
        assert!(!in_snap(Path::new("/snap/ghostty")));
        assert!(in_snap(Path::new("/snap/ghostty/820")));
    }
}
