//! Backups' side of `PapaApp`: saving one, and replacing the vault with one.
//!
//! ```text
//!  restore:  pick a file → read its header → confirm (and its password)
//!            → copy it next to the vault and check it   (vault still open)
//!            → close the vault → swap the files → open the restored one
//! ```
//!
//! Everything that can fail on a bad backup happens while the current vault
//! is still open, so a wrong password or a damaged file changes nothing.

use std::path::{Path, PathBuf};

use gpui_kit::component::{
    WindowExt as _,
    button::{Button, ButtonVariant, ButtonVariants as _},
    notification::Notification,
};
use gpui_kit::{AppContext as _, Context, Entity, PathPromptOptions, SharedString, Window};
use jiff::Zoned;
use jiff::tz::TimeZone;
use papa_core::backup::{self, BackupInfo};
use papa_core::lock::Password;
use papa_core::paths;

use super::{ExportBackup, Opened, PapaApp, RestoreBackup, Vault, open_vault};
use crate::export::unused_path;
use crate::ui::{PasswordForm, PasswordPurpose, dialogs::password_dialog, sentence};

/// Identifies the backup notifications, so a new one replaces the last.
struct BackupNotification;

impl PapaApp {
    /// Asks where to save, then writes a backup there.
    pub(super) fn export_backup(
        &mut self,
        _: &ExportBackup,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Vault::Open { store, .. } = &self.vault else {
            return;
        };
        let store = store.clone();
        // What's been typed goes into the backup: the database thread
        // stores the save before it makes the copy.
        if let Some(editor) = &self.editor {
            editor.update(cx, |editor, cx| editor.flush(window, cx));
        }
        let name = backup::file_name(Zoned::now().date());
        let dir = paths::export_dir();
        let chosen = cx.prompt_for_new_path(&dir, Some(&name));

        cx.spawn_in(window, async move |this, cx| {
            let (path, no_dialog) = match chosen.await {
                Ok(Ok(Some(path))) => (path, false),
                Ok(Ok(None)) | Err(_) => return,
                // No save dialog on this desktop: as for PDFs, save in
                // Documents under a free name.
                Ok(Err(_)) => (unused_path(&dir, &name, Path::exists), true),
            };
            this.update(cx, |this, cx| {
                this.exporting = Some("Backing up…".into());
                cx.notify();
            })
            .ok();
            let result = store.backup(&path).await;
            this.update_in(cx, |this, window, cx| {
                this.exporting = None;
                cx.notify();
                let notification = match result {
                    Ok(()) => {
                        let mut message = format!("Saved to {}.", paths::display(&path));
                        if no_dialog {
                            message.push_str(" There was no save dialog to ask where.");
                        }
                        Notification::success(message)
                            .title("Backed up your vault")
                            .action(move |_, _, _| {
                                let path = path.clone();
                                Button::new("show-backup")
                                    .primary()
                                    .label("Show in folder")
                                    .on_click(move |_, _, cx| cx.reveal_path(&path))
                            })
                    }
                    Err(err) => Notification::error(sentence(err)).title("Couldn't back up"),
                };
                window.push_notification(notification.id::<BackupNotification>(), cx);
            })
            .ok();
        })
        .detach();
    }

    /// Asks for a backup, then what to do with it. Works while the vault
    /// is locked or couldn't be opened too: that's when a backup helps most.
    pub(super) fn restore_backup(
        &mut self,
        _: &RestoreBackup,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if matches!(self.vault, Vault::Opening) {
            return;
        }
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Restore".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let path = match picked.await {
                Ok(Ok(Some(paths))) => paths.into_iter().next(),
                Ok(Ok(None)) | Err(_) => None,
                Ok(Err(err)) => {
                    this.update_in(cx, |_, window, cx| {
                        window.push_notification(
                            Notification::error(sentence(format!("{err:#}")))
                                .title("Couldn't open the file picker"),
                            cx,
                        )
                    })
                    .ok();
                    None
                }
            };
            let Some(path) = path else { return };
            let info = cx
                .background_spawn({
                    let path = path.clone();
                    async move { backup::inspect(&path) }
                })
                .await;
            this.update_in(cx, |this, window, cx| match info {
                Ok(info) => this.confirm_restore(path, info, window, cx),
                Err(err) => window.push_notification(
                    Notification::error(sentence(err)).title("Couldn't restore"),
                    cx,
                ),
            })
            .ok();
        })
        .detach();
    }

    /// Asks before replacing the vault, with the backup's password if it
    /// has one.
    fn confirm_restore(
        &mut self,
        backup: PathBuf,
        info: BackupInfo,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let when = info
            .created_at
            .to_zoned(TimeZone::system())
            .strftime("%b %-d %Y, %H:%M");
        let note = SharedString::from(format!(
            "The backup from {when} will replace your vault. Anything written since then \
             is kept only in the vault it replaces, which Papa moves aside, next to it."
        ));
        let this = cx.entity().downgrade();
        if info.encrypted {
            password_dialog(
                PasswordPurpose::Restore,
                Some(note),
                move |entry, form, window, cx| {
                    this.update(cx, |this, cx| {
                        this.restore(backup.clone(), entry.current, Some(form), window, cx)
                    })
                    .ok();
                },
                window,
                cx,
            );
        } else {
            window.open_alert_dialog(cx, move |alert, _, _| {
                let (this, backup) = (this.clone(), backup.clone());
                alert
                    .title("Replace your vault?")
                    .description(note.clone())
                    .confirm()
                    .ok_text("Replace vault")
                    .ok_variant(ButtonVariant::Danger)
                    .on_ok(move |_, window, cx| {
                        this.update(cx, |this, cx| {
                            this.restore(backup.clone(), None, None, window, cx)
                        })
                        .ok();
                        true
                    })
            });
        }
    }

    /// Checks the backup, then swaps it in for the vault and opens it.
    /// `form` is the password dialog, which shows a wrong password.
    fn restore(
        &mut self,
        backup: PathBuf,
        password: Option<Password>,
        form: Option<Entity<PasswordForm>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let vault = match paths::vault_path() {
            Ok(vault) => vault,
            Err(err) => {
                window.push_notification(
                    Notification::error(sentence(err)).title("Couldn't restore"),
                    cx,
                );
                return;
            }
        };
        cx.spawn_in(window, async move |this, cx| {
            // Copying and checking reads the whole backup: off the UI thread.
            let prepared = cx
                .background_spawn({
                    let (backup, vault, password) = (backup, vault.clone(), password.clone());
                    async move { backup::prepare_restore(&backup, &vault, password.as_ref()).await }
                })
                .await;
            let prepared = match prepared {
                Ok(prepared) => prepared,
                Err(err) => {
                    this.update_in(cx, |_, window, cx| match &form {
                        Some(form) => {
                            form.update(cx, |form, cx| form.set_error(sentence(err), cx));
                            crate::ui::dialogs::redraw_dialogs(window, cx);
                        }
                        None => window.push_notification(
                            Notification::error(sentence(err)).title("Couldn't restore"),
                            cx,
                        ),
                    })
                    .ok();
                    return;
                }
            };
            let kept = prepared.kept_path();

            let Ok(closing) = this.update_in(cx, |this, window, cx| {
                if form.is_some() {
                    window.close_dialog(cx);
                }
                this.close_vault(window, cx)
            }) else {
                return;
            };
            closing.await;
            let committed = cx.background_spawn(async move { prepared.commit() }).await;
            // Whichever vault is in place now, open it: the restored one
            // with the backup's password, or, if the swap failed, the old
            // one (which asks for its own).
            let password = committed.is_ok().then_some(password).flatten();
            let opened = open_vault(vault, password).await;

            this.update_in(cx, |this, window, cx| {
                let restored = committed.is_ok() && matches!(opened, Ok(Opened::Open(_)));
                this.vault_opened(opened, window, cx);
                let notification = match committed {
                    Ok(()) if restored => Notification::success(format!(
                        "Your previous vault is kept at {}.",
                        paths::display(&kept)
                    ))
                    .title("Restored the backup"),
                    Ok(()) => return,
                    Err(err) => Notification::error(sentence(err))
                        .title("Couldn't restore")
                        .autohide(false),
                };
                window.push_notification(notification.id::<BackupNotification>(), cx);
            })
            .ok();
        })
        .detach();
    }
}
