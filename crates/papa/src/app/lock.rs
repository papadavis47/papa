//! The vault's lock: the unlock screen, locking (by hand or after a while
//! idle), and setting, changing or removing the password.
//!
//! ```text
//!  Opening ──plain──────────────────────────────▶ Open { encrypted: false }
//!     │                                              │  ▲ set / remove password
//!     └─encrypted─▶ Locked ──password─▶ Open { encrypted: true }
//!                     ▲                              │
//!                     └──── ⌘L, or idle too long ────┘
//! ```
//!
//! Locking closes the store, which drops Turso's copy of the key, and drops
//! the editor, the lists and the selection: nothing of the vault stays on
//! screen, and as little as possible in memory.

use std::future::Future;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::component::{WindowExt as _, notification::Notification};
use gpui_kit::{AppContext as _, Context, Entity, Global, SharedString, Task, Window};
use papa_core::Error;

use super::{ChangePassword, LockVault, PapaApp, RemovePassword, Vault, open_vault};
use crate::settings::AppSettings;
use crate::state::Selection;
use crate::ui::dialogs::password_dialog;
use crate::ui::{
    PasswordEntry, PasswordForm, PasswordPurpose, UnlockEvent, UnlockScreen, sentence,
};

/// How often auto-lock checks the time. A lock can come this much late.
const IDLE_CHECK: Duration = Duration::from_secs(5);

/// The vault's state as the Settings window needs it: which password
/// buttons to offer. A global, so that window can read and observe it
/// without knowing about `PapaApp`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VaultStatus {
    /// Opening, failed, or being replaced.
    #[default]
    Unavailable,
    Locked,
    Open {
        encrypted: bool,
    },
}

impl Global for VaultStatus {}

/// Whether a vault idle for `idle` should lock, with auto-lock set to
/// `minutes` (0: never).
pub fn idle_too_long(idle: Duration, minutes: u32) -> bool {
    minutes > 0 && idle >= Duration::from_secs(u64::from(minutes) * 60)
}

impl PapaApp {
    /// Replaces the vault's state, and publishes it for the Settings window.
    pub(super) fn set_vault(&mut self, vault: Vault, cx: &mut Context<Self>) {
        let status = match &vault {
            Vault::Open { encrypted, .. } => VaultStatus::Open {
                encrypted: *encrypted,
            },
            Vault::Locked { .. } => VaultStatus::Locked,
            Vault::Opening | Vault::Failed => VaultStatus::Unavailable,
        };
        self.vault = vault;
        cx.set_global(status);
        cx.notify();
    }

    /// Closes the vault and forgets everything it showed. The returned
    /// future saves what was being typed, then closes the store; the
    /// caller decides what comes next and awaits it.
    pub(super) fn close_vault(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl Future<Output = ()> + use<> {
        // Answers still on their way belong to this vault: ignore them.
        self.generation += 1;
        let save = self.editor.take().map(|editor| {
            editor.update(cx, |editor, cx| {
                let save = editor.save_before_closing(cx);
                // Releases the note's images from GPUI's cache too.
                editor.close(window, cx);
                save
            })
        });
        self.editor_subscription = None;
        // Sharing's thread stops once the vault's `Sharing` is dropped,
        // below, and the sync task's clone with it.
        self.sync_task = None;
        self.share_dialog = None;
        self.account = None;
        self.shares = Rc::default();
        self.sync_status = super::sharing::SyncStatus::default();
        self.children = Rc::default();
        self.labels = Rc::default();
        self.notes = Rc::default();
        self.selection = Selection::default();
        self.search_task = None;
        self.search
            .update(cx, |search, cx| search.set_value("", window, cx));
        if self.palette.read(cx).is_open() {
            self.palette
                .update(cx, |palette, cx| palette.close(window, cx));
        }
        let store = match &self.vault {
            Vault::Open { store, .. } => Some(store.clone()),
            _ => None,
        };
        self.set_vault(Vault::Opening, cx);
        self.sharing_changed(cx);
        async move {
            if let Some(save) = save {
                save.await;
            }
            if let Some(store) = store {
                // Requests sent before this one are answered first: the
                // save above is stored before the vault closes.
                let _ = store.close().await;
            }
        }
    }

    /// ⌘L: locks a vault that has a password.
    pub(super) fn lock_vault(
        &mut self,
        _: &LockVault,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Vault::Open {
            path,
            encrypted: true,
            ..
        } = &self.vault
        else {
            return;
        };
        let path = path.clone();
        let closing = self.close_vault(window, cx);
        // Shown at once: closing finishes in the background.
        self.show_locked(path, window, cx);
        cx.background_spawn(closing).detach();
    }

    /// Shows the unlock screen for the vault at `path`.
    pub(super) fn show_locked(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let screen = cx.new(|cx| UnlockScreen::new(window, cx));
        self.unlock_subscription = Some(cx.subscribe_in(&screen, window, Self::on_unlock_event));
        screen.update(cx, |screen, cx| screen.focus(window, cx));
        self.set_vault(Vault::Locked { path, screen }, cx);
    }

    fn on_unlock_event(
        &mut self,
        screen: &Entity<UnlockScreen>,
        event: &UnlockEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let UnlockEvent::Submit(password) = event;
        let Vault::Locked { path, .. } = &self.vault else {
            return;
        };
        let (path, password, screen) = (path.clone(), password.clone(), screen.clone());
        cx.spawn_in(window, async move |this, cx| {
            let opened = open_vault(path, Some(password)).await;
            this.update_in(cx, |this, window, cx| match opened {
                Ok(opened) => this.vault_opened(Ok(opened), window, cx),
                Err(err) => {
                    // The likeliest mistake gets a short message; anything
                    // else, the whole story.
                    let message = match err.downcast_ref::<Error>() {
                        Some(Error::WrongPassword) => "That isn't the password.".to_owned(),
                        _ => sentence(format!("{err:#}")),
                    };
                    screen.update(cx, |screen, cx| screen.failed(message, window, cx));
                }
            })
        })
        .detach();
    }

    /// Checks every few seconds whether the vault has been idle long
    /// enough to lock. A task rather than a timer reset by every key and
    /// mouse move: those only write down the time.
    pub(super) fn watch_idle(window: &mut Window, cx: &mut Context<Self>) -> Task<()> {
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(IDLE_CHECK).await;
                let checked = this.update_in(cx, |this, window, cx| {
                    let minutes = AppSettings::get(cx).privacy.auto_lock_minutes;
                    if idle_too_long(this.last_activity.elapsed(), minutes) {
                        this.lock_vault(&LockVault, window, cx);
                    }
                });
                // The window is gone.
                if checked.is_err() {
                    break;
                }
            }
        })
    }

    /// Opens the dialog to set a first password, or to change it.
    pub(super) fn change_password(
        &mut self,
        _: &ChangePassword,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Vault::Open { encrypted, .. } = self.vault else {
            return;
        };
        let (purpose, note) = if encrypted {
            (PasswordPurpose::Change, None)
        } else {
            (
                PasswordPurpose::Set,
                Some(
                    "Papa will encrypt your vault and ask for this password each time it \
                     opens. Nobody can recover a forgotten password, not even with a backup \
                     made after today, so keep it somewhere safe."
                        .into(),
                ),
            )
        };
        self.password_dialog(purpose, note, window, cx);
    }

    pub(super) fn remove_password(
        &mut self,
        _: &RemovePassword,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Vault::Open {
            encrypted: true, ..
        } = self.vault
        else {
            return;
        };
        let note = "Papa will decrypt your vault. Anyone who can read your files will be able \
                    to read your notes.";
        self.password_dialog(PasswordPurpose::Remove, Some(note.into()), window, cx);
    }

    fn password_dialog(
        &mut self,
        purpose: PasswordPurpose,
        note: Option<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let this = cx.entity().downgrade();
        password_dialog(
            purpose,
            note,
            move |entry, form, window, cx| {
                this.update(cx, |this, cx| {
                    this.password_submitted(purpose, entry, form, window, cx)
                })
                .ok();
            },
            window,
            cx,
        );
    }

    /// Runs what the password dialog asked for. The dialog stays open,
    /// busy, until the vault has done it.
    fn password_submitted(
        &mut self,
        purpose: PasswordPurpose,
        entry: PasswordEntry,
        form: Entity<PasswordForm>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Vault::Open { store, .. } = &self.vault else {
            return;
        };
        let store = store.clone();
        cx.spawn_in(window, async move |this, cx| {
            let (result, encrypted, done) = match (purpose, entry) {
                (
                    PasswordPurpose::Set | PasswordPurpose::Change,
                    PasswordEntry {
                        current,
                        new: Some(new),
                    },
                ) => {
                    let done = if purpose == PasswordPurpose::Set {
                        "Your vault is encrypted. Papa will ask for the password when it opens."
                    } else {
                        "Papa will ask for the new password from now on."
                    };
                    (store.set_password(current, new).await, true, done)
                }
                (
                    PasswordPurpose::Remove,
                    PasswordEntry {
                        current: Some(current),
                        ..
                    },
                ) => (
                    store.remove_password(current).await,
                    false,
                    "Your vault is no longer encrypted.",
                ),
                _ => return,
            };
            this.update_in(cx, |this, window, cx| match result {
                Ok(()) => {
                    window.close_dialog(cx);
                    if let Vault::Open {
                        store,
                        path,
                        sharing,
                        ..
                    } = &this.vault
                    {
                        let vault = Vault::Open {
                            store: store.clone(),
                            path: path.clone(),
                            encrypted,
                            sharing: sharing.clone(),
                        };
                        this.set_vault(vault, cx);
                    }
                    let title = match purpose {
                        PasswordPurpose::Set => "Vault encrypted",
                        PasswordPurpose::Remove => "Vault decrypted",
                        _ => "Password changed",
                    };
                    window.push_notification(Notification::success(done).title(title), cx);
                }
                Err(err) => {
                    form.update(cx, |form, cx| form.set_error(sentence(err), cx));
                    crate::ui::dialogs::redraw_dialogs(window, cx);
                }
            })
            .ok();
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_lock_waits_its_minutes() {
        assert!(!idle_too_long(Duration::from_secs(14 * 60 + 59), 15));
        assert!(idle_too_long(Duration::from_secs(15 * 60), 15));
        assert!(idle_too_long(Duration::from_secs(61), 1));
    }

    #[test]
    fn zero_minutes_never_locks() {
        assert!(!idle_too_long(Duration::from_secs(365 * 24 * 3600), 0));
    }
}
