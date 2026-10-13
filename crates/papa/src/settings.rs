//! The settings as a GPUI global: read from anywhere, observed for changes,
//! saved to `settings.json` shortly after each change.
//!
//! A global is one value per type for the whole app, stored in `App`. Any
//! view reads it with `AppSettings::get(cx)`; `cx.observe_global::<AppSettings>`
//! runs a callback after every change. That's how a choice made in the
//! Settings window restyles the main window: neither window knows the other.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::Context as _;
use gpui_kit::{App, Global, Task};
use papa_core::paths;
use papa_core::settings::Settings;

/// How long after the last change the file is written. A burst of changes,
/// such as stepping the text size up five times, is one write.
const SAVE_DELAY: Duration = Duration::from_millis(300);

pub struct AppSettings {
    settings: Settings,
    /// `None` if the OS has no config folder: changes then last until quit.
    path: Option<PathBuf>,
}

impl Global for AppSettings {}

/// The save waiting for [`SAVE_DELAY`]. A global of its own, because
/// touching `AppSettings` mutably notifies its observers: replacing the task
/// there would look like a settings change.
#[derive(Default)]
struct PendingSave {
    /// Held, not read: dropping it would cancel the save.
    _task: Option<Task<()>>,
    /// Changed since the last save started.
    dirty: bool,
}

impl Global for PendingSave {}

/// Loads the settings into the global. Call once at startup, before
/// `theme::init` and before any window opens, so the first frame already
/// uses them.
///
/// Reading a small file on the UI thread is fine this early: nothing is on
/// screen yet to stutter. A file that can't be read gives the defaults; the
/// error comes back for the window to show, and the file is left alone
/// until the father changes a setting.
pub fn init(cx: &mut App) -> Option<anyhow::Error> {
    let (settings, path, problem) = match load() {
        Ok((settings, path)) => (settings, Some(path), None),
        Err((path, err)) => (Settings::default(), path, Some(err)),
    };
    cx.set_global(AppSettings { settings, path });
    cx.set_global(PendingSave::default());

    // Write a change still waiting for its timer. GPUI waits for the
    // returned future only briefly, so the write happens right here.
    cx.on_app_quit(|cx| {
        if std::mem::take(&mut cx.global_mut::<PendingSave>().dirty) {
            let this = cx.global::<AppSettings>();
            if let Some(path) = &this.path
                && let Err(err) = this.settings.save(path)
            {
                eprintln!("papa: {err}");
            }
        }
        async {}
    })
    .detach();
    problem
}

type LoadError = (Option<PathBuf>, anyhow::Error);

fn load() -> Result<(Settings, PathBuf), LoadError> {
    let path = paths::settings_path().map_err(|err| (None, err.into()))?;
    let settings = Settings::load(&path)
        .context("Papa is using the default settings for now")
        .map_err(|err| (Some(path.clone()), err))?;
    Ok((settings, path))
}

impl AppSettings {
    pub fn get(cx: &App) -> &Settings {
        &cx.global::<Self>().settings
    }

    /// Where the settings are saved, if anywhere.
    pub fn path(cx: &App) -> Option<&PathBuf> {
        cx.global::<Self>().path.as_ref()
    }

    /// Changes the settings with `edit`, then saves them shortly after.
    ///
    /// The result is normalized, so a value out of range is clamped rather
    /// than stored. Observers hear only of real changes: setting a value it
    /// already has does nothing.
    pub fn update(cx: &mut App, edit: impl FnOnce(&mut Settings)) {
        let current = Self::get(cx);
        let mut next = current.clone();
        edit(&mut next);
        let next = next.normalized();
        if next == *current {
            return;
        }
        cx.global_mut::<Self>().settings = next;
        schedule_save(cx);
    }
}

/// Saves the settings after [`SAVE_DELAY`], replacing any save already
/// waiting. Dropping a GPUI task cancels it, so only the latest one runs.
fn schedule_save(cx: &mut App) {
    let Some(path) = AppSettings::path(cx).cloned() else {
        return;
    };
    let task = cx.spawn(async move |cx| {
        cx.background_executor().timer(SAVE_DELAY).await;
        let settings = cx.update(|cx| {
            cx.global_mut::<PendingSave>().dirty = false;
            AppSettings::get(cx).clone()
        });
        // File I/O goes to the background pool, off the UI thread.
        let saved = cx
            .background_executor()
            .spawn(async move { settings.save(&path) })
            .await;
        if let Err(err) = saved {
            cx.update(|cx| crate::ui::notify_error("Couldn't save your settings", err, cx));
        }
    });
    // Not cleared when the task finishes: a task dropping its own handle
    // would cancel itself mid-save. It's replaced by the next change instead.
    *cx.global_mut::<PendingSave>() = PendingSave {
        _task: Some(task),
        dirty: true,
    };
}
