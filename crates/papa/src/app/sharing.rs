//! Sharing, in the main window: the father's account, the sharing dialog
//! of each child, and syncing shared notes in the background.
//!
//! The work happens on `papa-core`'s sharing thread ([`Sharing`]); this
//! module decides when to ask for it and shows the results. Syncs are
//! debounced like autosave: a change schedules one a few seconds later,
//! and another change before then pushes it back.

use std::rc::Rc;
use std::time::Duration;

use gpui_kit::component::{WindowExt as _, notification::Notification};
use gpui_kit::{
    AppContext as _, Context, Entity, Global, ParentElement as _, SharedString, Window, px,
};
use jiff::Timestamp;
use papa_core::relay::DEFAULT_RELAY_URL;
use papa_core::{ChildId, ChildShare, Error, Sharing, SyncReport};
use papa_proto::ErrorCode;

use super::{CreateAccount, PapaApp, SignIn, SignOut, SyncNow, Vault};
use crate::ui::dialogs::{account_dialog, redraw_dialogs};
use crate::ui::{
    AccountEntry, AccountForm, AccountPurpose, ShareDialog, ShareDialogEvent, ShareState, sentence,
};

/// How long after the last change a sync starts. Typing saves every half
/// second; this waits for a pause.
const SYNC_DELAY: Duration = Duration::from_secs(3);

/// Where syncing shared notes stands, for the status bar.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum SyncStatus {
    /// Nothing synced since the vault opened.
    #[default]
    Idle,
    Syncing,
    Synced(Timestamp),
    Failed(String),
}

/// The account as the Settings window shows it. A global, like
/// [`VaultStatus`](super::VaultStatus): that window reads and observes it
/// without knowing about `PapaApp`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AccountStatus {
    pub vault_open: bool,
    /// The email and server signed in to.
    pub signed_in: Option<(String, String)>,
    /// How many children's sharing is on.
    pub shares: usize,
}

impl Global for AccountStatus {}

/// Whether `err` means the father has to sign in (again).
fn signed_out(err: &Error) -> bool {
    matches!(
        err,
        Error::NotSignedIn
            | Error::Relay {
                code: ErrorCode::Unauthorized,
                ..
            }
    )
}

impl PapaApp {
    fn sharing(&self) -> Option<Sharing> {
        match &self.vault {
            Vault::Open { sharing, .. } => Some(sharing.clone()),
            _ => None,
        }
    }

    /// The children whose sharing is on.
    pub(super) fn shared_children(&self) -> Vec<ChildId> {
        self.shares.iter().map(|share| share.child_id).collect()
    }

    /// Publishes the account for the Settings window, and tells the editor
    /// and the share dialog what changed.
    pub(super) fn sharing_changed(&mut self, cx: &mut Context<Self>) {
        let status = AccountStatus {
            vault_open: self.is_open(),
            signed_in: self
                .account
                .as_ref()
                .map(|account| (account.email.clone(), account.relay_url.clone())),
            shares: self.shares.len(),
        };
        cx.set_global(status);
        if let Some(editor) = &self.editor {
            let shared = Rc::new(self.shared_children());
            editor.update(cx, |editor, cx| editor.set_shared_children(shared, cx));
        }
        if let Some((dialog, _)) = &self.share_dialog {
            let child = dialog.read(cx).child().id;
            let state = self.share_state(child);
            dialog.update(cx, |dialog, cx| dialog.set_state(state, cx));
        }
        cx.notify();
    }

    /// Reloads the account and the shares from the vault.
    fn reload_sharing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.with_store(
            "Couldn't load your sharing",
            async move |store| Ok((store.account().await?, store.shares().await?)),
            |this, (account, shares), _, cx| {
                this.account = account;
                this.shares = Rc::new(shares);
                this.sharing_changed(cx);
            },
            window,
            cx,
        );
    }

    // --- Syncing ---

    /// Syncs every share after `delay`, unless another change comes first.
    pub(super) fn schedule_sync(
        &mut self,
        delay: Duration,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(sharing) = self.sharing() else {
            return;
        };
        if self.account.is_none() || self.shares.is_empty() {
            return;
        }
        let generation = self.generation;
        // Replacing the task cancels a sync still waiting for its delay. One
        // already running finishes on the sharing thread; this one queues
        // after it there.
        self.sync_task = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(delay).await;
            let started = this.update(cx, |this, cx| {
                this.sync_status = SyncStatus::Syncing;
                cx.notify();
            });
            if started.is_err() {
                return;
            }
            let results = sharing.sync_all().await;
            this.update_in(cx, |this, window, cx| {
                if this.generation == generation {
                    this.synced(results, window, cx);
                }
            })
            .ok();
        }));
    }

    fn synced(
        &mut self,
        results: papa_core::Result<Vec<(ChildId, papa_core::Result<SyncReport>)>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let failure = match results {
            Err(err) => Some(sentence(err)),
            Ok(results) => {
                let mut failures = results.into_iter().filter_map(|(child, result)| {
                    let err = result.err()?;
                    let name = self.child(child).map_or("a child", |c| c.name.as_str());
                    Some((signed_out(&err), format!("{name}: {}", sentence(err))))
                });
                let first = failures.next();
                if first.as_ref().is_some_and(|(signed_out, _)| *signed_out) {
                    self.account = None;
                }
                first.map(|(_, message)| message)
            }
        };
        self.sync_status = match failure {
            Some(message) => SyncStatus::Failed(message),
            None => SyncStatus::Synced(Timestamp::now()),
        };
        // For each share's "last synced".
        self.reload_sharing(window, cx);
    }

    pub(super) fn sync_now(&mut self, _: &SyncNow, window: &mut Window, cx: &mut Context<Self>) {
        if self.account.is_none() {
            window.push_notification(
                Notification::info("Sign in to your Papa account first, in Settings → Sharing.")
                    .title("Not signed in"),
                cx,
            );
            return;
        }
        self.schedule_sync(Duration::ZERO, window, cx);
    }

    /// After a note changed: sync soon, if any note is shared at all.
    pub(super) fn note_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.schedule_sync(SYNC_DELAY, window, cx);
    }

    /// For the status bar.
    pub(super) fn sync_label(&self) -> Option<SharedString> {
        if self.shares.is_empty() {
            return None;
        }
        Some(match (&self.sync_status, &self.account) {
            (_, None) => "Sharing paused: signed out".into(),
            (SyncStatus::Idle, _) => "Shared notes".into(),
            (SyncStatus::Syncing, _) => "Syncing shared notes…".into(),
            (SyncStatus::Synced(at), _) => format!(
                "Shared notes synced {}",
                crate::ui::dates::ago(*at, Timestamp::now())
            )
            .into(),
            (SyncStatus::Failed(_), _) => "Sync failed".into(),
        })
    }

    // --- The account ---

    pub(super) fn sign_in(&mut self, _: &SignIn, window: &mut Window, cx: &mut Context<Self>) {
        self.account_dialog(AccountPurpose::SignIn, window, cx);
    }

    pub(super) fn create_account(
        &mut self,
        _: &CreateAccount,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.account_dialog(AccountPurpose::SignUp, window, cx);
    }

    fn account_dialog(
        &mut self,
        purpose: AccountPurpose,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.is_open() {
            return;
        }
        let (server, email) = match &self.account {
            Some(account) => (account.relay_url.clone(), account.email.clone()),
            None => (DEFAULT_RELAY_URL.to_owned(), String::new()),
        };
        let this = cx.entity().downgrade();
        account_dialog(
            purpose,
            server,
            email,
            move |entry, form, window, cx| {
                this.update(cx, |this, cx| {
                    this.account_submitted(purpose, entry, form, window, cx)
                })
                .ok();
            },
            window,
            cx,
        );
    }

    fn account_submitted(
        &mut self,
        purpose: AccountPurpose,
        entry: AccountEntry,
        form: Entity<AccountForm>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(sharing) = self.sharing() else {
            return;
        };
        let generation = self.generation;
        cx.spawn_in(window, async move |this, cx| {
            let AccountEntry {
                relay_url,
                invite_code,
                email,
                password,
            } = entry;
            let result = match (purpose, invite_code) {
                (AccountPurpose::SignUp, Some(code)) => {
                    sharing.sign_up(relay_url, code, email, password).await
                }
                _ => sharing.log_in(relay_url, email, password).await,
            };
            this.update_in(cx, |this, window, cx| {
                if this.generation != generation {
                    return;
                }
                match result {
                    Ok(account) => {
                        window.close_dialog(cx);
                        window.push_notification(
                            Notification::success(format!(
                                "Signed in as {} on {}.",
                                account.email, account.relay_url
                            ))
                            .title("Signed in"),
                            cx,
                        );
                        this.account = Some(account);
                        this.sharing_changed(cx);
                        this.schedule_sync(Duration::ZERO, window, cx);
                    }
                    Err(err) => {
                        form.update(cx, |form, cx| form.set_error(sentence(err), cx));
                        redraw_dialogs(window, cx);
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn sign_out(&mut self, _: &SignOut, window: &mut Window, cx: &mut Context<Self>) {
        let Some(sharing) = self.sharing() else {
            return;
        };
        self.sync_task = None;
        let generation = self.generation;
        cx.spawn_in(window, async move |this, cx| {
            let result = sharing.log_out().await;
            this.update_in(cx, |this, window, cx| {
                if this.generation != generation {
                    return;
                }
                match result {
                    Ok(()) => {
                        this.account = None;
                        this.sync_status = SyncStatus::Idle;
                        this.sharing_changed(cx);
                        let note = if this.shares.is_empty() {
                            "You can sign in again in Settings → Sharing."
                        } else {
                            "Shared notes stay shared. Papa syncs them again once you sign in."
                        };
                        window.push_notification(Notification::info(note).title("Signed out"), cx);
                    }
                    Err(err) => window.push_notification(
                        Notification::error(sentence(err)).title("Couldn't sign out"),
                        cx,
                    ),
                }
            })
            .ok();
        })
        .detach();
    }

    // --- A child's sharing dialog ---

    fn share_state(&self, child: ChildId) -> ShareState {
        let share = self.shares.iter().find(|share| share.child_id == child);
        match (&self.account, share) {
            (None, _) => ShareState::SignedOut,
            (Some(_), None) => ShareState::Off,
            (Some(account), Some(share)) => ShareState::On {
                share: Box::new(share.clone()),
                email: account.email.clone(),
            },
        }
    }

    pub(super) fn open_share_dialog(
        &mut self,
        child: ChildId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(child) = self.child(child).cloned() else {
            return;
        };
        let title = format!("Sharing with {}", child.name);
        let state = self.share_state(child.id);
        let dialog = cx.new(|cx| ShareDialog::new(child, cx));
        dialog.update(cx, |dialog, cx| dialog.set_state(state, cx));
        let subscription = cx.subscribe_in(&dialog, window, Self::on_share_dialog_event);
        self.share_dialog = Some((dialog.clone(), subscription));

        let content = dialog.clone();
        window.open_dialog(cx, move |modal, _, cx| {
            let dialog = content.clone();
            let busy = dialog.read(cx).is_busy();
            modal
                .title(title.clone())
                .w(px(480.))
                .close_button(!busy)
                .keyboard(!busy)
                .overlay_closable(!busy)
                .child(dialog.clone())
                // Enter would close the dialog; it does what the dialog asks
                // for instead. Esc closes it.
                .on_ok({
                    let dialog = dialog.clone();
                    move |_, _, cx| {
                        dialog.update(cx, |dialog, cx| dialog.primary(cx));
                        false
                    }
                })
        });
        dialog.update(cx, |dialog, cx| dialog.keep_focus(window, cx));
    }

    fn on_share_dialog_event(
        &mut self,
        dialog: &Entity<ShareDialog>,
        event: &ShareDialogEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(sharing) = self.sharing() else {
            return;
        };
        let child = dialog.read(cx).child().clone();
        let name = child.name.clone();
        let busy = match event {
            ShareDialogEvent::SignIn => {
                window.close_dialog(cx);
                self.sign_in(&SignIn, window, cx);
                return;
            }
            ShareDialogEvent::Start => format!("Setting up {name}'s share on the server…"),
            ShareDialogEvent::NewKey => format!("Making a new key for {name}…"),
            ShareDialogEvent::Stop => format!("Deleting {name}'s copy on the server…"),
            ShareDialogEvent::SyncNow => format!("Syncing {name}'s notes…"),
        };
        dialog.update(cx, |dialog, cx| dialog.set_busy(busy, cx));
        redraw_dialogs(window, cx);

        let event = *event;
        let dialog = dialog.clone();
        let generation = self.generation;
        cx.spawn_in(window, async move |this, cx| {
            // A new or rekeyed share syncs at once, so the child can sign
            // in straight away and the old key's copy is resealed.
            let result: papa_core::Result<Option<ChildShare>> = match event {
                ShareDialogEvent::Start => match sharing.start_sharing(child.id).await {
                    Ok(share) => sharing.sync(child.id).await.map(|_| Some(share)),
                    Err(err) => Err(err),
                },
                ShareDialogEvent::NewKey => match sharing.new_key(child.id).await {
                    Ok(share) => sharing.sync(child.id).await.map(|_| Some(share)),
                    Err(err) => Err(err),
                },
                ShareDialogEvent::Stop => sharing.stop_sharing(child.id).await.map(|()| None),
                ShareDialogEvent::SyncNow => sharing.sync(child.id).await.map(|_| None),
                ShareDialogEvent::SignIn => return,
            };
            this.update_in(cx, |this, window, cx| {
                if this.generation != generation {
                    return;
                }
                match result {
                    Ok(_) => {
                        if event != ShareDialogEvent::Stop {
                            this.sync_status = SyncStatus::Synced(Timestamp::now());
                        }
                        if event == ShareDialogEvent::Stop
                            && let Some(editor) = &this.editor
                        {
                            editor.update(cx, |editor, cx| editor.forget_share(child.id, cx));
                        }
                    }
                    Err(err) => {
                        if signed_out(&err) {
                            this.account = None;
                        }
                        dialog.update(cx, |dialog, cx| dialog.set_error(sentence(err), cx));
                    }
                }
                // The state shown comes from the vault either way: a share
                // made before a failed sync is there, and shown.
                this.reload_sharing(window, cx);
                dialog.update(cx, |dialog, cx| dialog.keep_focus(window, cx));
                redraw_dialogs(window, cx);
            })
            .ok();
        })
        .detach();
    }
}
