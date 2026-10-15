//! The vault's storage, run as an actor on its own thread.
//!
//! ```text
//!  UI thread (GPUI)                      "papa-store" thread (tokio)
//!  ───────────────                       ───────────────────────────
//!  store.add_child(draft).await
//!    ├─ Request::AddChild(draft, reply) ──▶ inbox.recv()
//!    │                                      db.add_child(draft).await
//!    └─ reply.await  ◀──────────────────── reply.send(result)
//! ```
//!
//! [`Store`] is the handle: it only holds the sending end of a channel, so it
//! is cheap to clone and safe to move between threads. The thread owns the
//! database connection and handles one request at a time, in order. It
//! exits on [`Store::close`], or when the last `Store` clone is dropped and
//! the channel closes.

mod db;
mod migrations;
mod rekey;
mod shares;

use std::path::{Path, PathBuf};
use std::thread;

use tokio::sync::{mpsc, oneshot};

use crate::lock::{self, KeyFile, Password, VaultKey, VaultKind, key_path};
use crate::share::projection::{Manifest, ShareChanges};
use crate::share::{AccessKey, ContentKey, Sealed};
use crate::{
    Account, Child, ChildDraft, ChildId, ChildShare, EncodedImage, Error, ImageId, Label, LabelId,
    Note, NoteDraft, NoteId, NoteImage, NoteQuery, NoteSummary, PreparedImage, Result,
};
use db::{Db, Location};

/// The schema version this build of Papa writes.
pub(crate) const LATEST_SCHEMA: i64 = migrations::LATEST;

/// A handle to the vault. Every method sends a request to the database thread
/// and waits for its reply, so they can be awaited from any async executor,
/// including GPUI's, without blocking it.
#[derive(Clone, Debug)]
pub struct Store {
    requests: mpsc::UnboundedSender<Request>,
}

/// Where the database thread sends a request's result.
type Reply<T> = oneshot::Sender<Result<T>>;

/// One message per [`Store`] method. Everything in it is owned (`String`, not
/// `&str`), because it moves to another thread and may outlive the caller's
/// borrows.
enum Request {
    Children(Reply<Vec<Child>>),
    AddChild(ChildDraft, Reply<Child>),
    UpdateChild(ChildId, ChildDraft, Reply<Child>),
    DeleteChild(ChildId, Reply<()>),
    Labels(Reply<Vec<Label>>),
    AddLabel(String, Reply<Label>),
    RenameLabel(LabelId, String, Reply<Label>),
    DeleteLabel(LabelId, Reply<()>),
    Notes(NoteQuery, Reply<Vec<NoteSummary>>),
    Note(NoteId, Reply<Note>),
    AddNote(NoteDraft, Reply<Note>),
    UpdateNoteText(NoteId, String, String, Reply<Note>),
    SetNoteChildren(NoteId, Vec<ChildId>, Reply<Note>),
    SetNoteLabels(NoteId, Vec<LabelId>, Reply<Note>),
    SetNoteShared(NoteId, ChildId, bool, Reply<Note>),
    DeleteNote(NoteId, Reply<()>),
    NoteImages(NoteId, Reply<Vec<NoteImage>>),
    // Boxed: a `PreparedImage` is much larger than the other variants, and
    // an enum is as large as its largest variant.
    AddImage(NoteId, Box<PreparedImage>, Reply<NoteImage>),
    DisplayImage(ImageId, Reply<EncodedImage>),
    DeleteImage(ImageId, Reply<()>),
    Account(Reply<Option<Account>>),
    SetAccount(Account, Reply<()>),
    ClearAccount(Reply<()>),
    Shares(Reply<Vec<ChildShare>>),
    Share(ChildId, Reply<Option<ChildShare>>),
    AddShare(Box<ChildShare>, ContentKey, Reply<()>),
    RekeyShare(Box<Rekey>, Reply<ChildShare>),
    AuthKeyUpdated(ChildId, i64, Reply<()>),
    ShareSynced(ChildId, jiff::Timestamp, Reply<()>),
    RemoveShare(ChildId, Reply<()>),
    ShareChanges(ChildId, Box<Manifest>, usize, Reply<ShareChanges>),
    IsEncrypted(Reply<bool>),
    SetPassword(Option<Password>, Password, Reply<()>),
    RemovePassword(Password, Reply<()>),
    Backup(PathBuf, Reply<()>),
    Check(Reply<()>),
    Close(Reply<()>),
}

/// A share's new keys, for [`Store::rekey_share`].
pub(crate) struct Rekey {
    pub child: ChildId,
    pub epoch: i64,
    pub access_key: AccessKey,
    pub content_key: ContentKey,
    pub wrapped: Sealed,
}

/// How the database thread gets the vault's key, if it needs one.
enum Unlock {
    /// The vault isn't encrypted.
    Plain,
    /// Derive the key from the password and the key file. Done on the
    /// database thread: Argon2id takes a moment, on purpose.
    Password(Password),
    Key(VaultKey),
}

impl Store {
    /// Opens the vault file at `path`, creating it and its folder if needed,
    /// and migrates it to the current schema.
    ///
    /// Fails with [`Error::Locked`] if the vault is encrypted: open it with
    /// [`unlock`](Self::unlock) instead.
    pub async fn open(path: impl Into<PathBuf>) -> Result<Self> {
        Self::start(Location::File(path.into()), Unlock::Plain).await
    }

    /// Opens an encrypted vault with its password. Fails with
    /// [`Error::WrongPassword`] if it isn't the right one.
    pub async fn unlock(path: impl Into<PathBuf>, password: Password) -> Result<Self> {
        Self::start(Location::File(path.into()), Unlock::Password(password)).await
    }

    /// Opens a vault whose key is already known (or plain, for `None`).
    pub(crate) async fn open_with_key(path: &Path, key: Option<VaultKey>) -> Result<Self> {
        let unlock = key.map_or(Unlock::Plain, Unlock::Key);
        Self::start(Location::File(path.to_owned()), unlock).await
    }

    /// A throwaway vault in memory: for tests, and later for demo mode.
    pub async fn open_in_memory() -> Result<Self> {
        Self::start(Location::Memory, Unlock::Plain).await
    }

    /// Spawns the database thread and waits until the vault is open.
    async fn start(location: Location, unlock: Unlock) -> Result<Self> {
        let (requests, inbox) = mpsc::unbounded_channel();
        let (opened, open_result) = oneshot::channel();
        thread::Builder::new()
            .name("papa-store".into())
            .spawn(move || serve(location, unlock, inbox, opened))
            .map_err(Error::StartThread)?;
        // The outer `?` covers the thread dying before it replied; the inner
        // one is the result of opening the vault.
        open_result.await.map_err(|_| Error::StoreClosed)??;
        Ok(Self { requests })
    }

    /// All children, in sidebar order.
    pub async fn children(&self) -> Result<Vec<Child>> {
        // A tuple variant's name is also a function from its fields to the
        // enum, so `Request::Children` is an `fn(Reply<_>) -> Request`.
        self.call(Request::Children).await
    }

    /// Adds a child at the end of the list.
    pub async fn add_child(&self, draft: ChildDraft) -> Result<Child> {
        self.call(|reply| Request::AddChild(draft, reply)).await
    }

    /// Replaces a child's name and colour.
    pub async fn update_child(&self, id: ChildId, draft: ChildDraft) -> Result<Child> {
        self.call(|reply| Request::UpdateChild(id, draft, reply))
            .await
    }

    pub async fn delete_child(&self, id: ChildId) -> Result<()> {
        self.call(|reply| Request::DeleteChild(id, reply)).await
    }

    /// All labels, alphabetically, ignoring case.
    pub async fn labels(&self) -> Result<Vec<Label>> {
        self.call(Request::Labels).await
    }

    /// Adds a label. Fails with [`Error::DuplicateLabel`] if the name is
    /// taken, ignoring case.
    pub async fn add_label(&self, name: impl Into<String>) -> Result<Label> {
        let name = name.into();
        self.call(|reply| Request::AddLabel(name, reply)).await
    }

    pub async fn rename_label(&self, id: LabelId, name: impl Into<String>) -> Result<Label> {
        let name = name.into();
        self.call(|reply| Request::RenameLabel(id, name, reply))
            .await
    }

    pub async fn delete_label(&self, id: LabelId) -> Result<()> {
        self.call(|reply| Request::DeleteLabel(id, reply)).await
    }

    /// The notes `query` asks for, newest first.
    pub async fn notes(&self, query: NoteQuery) -> Result<Vec<NoteSummary>> {
        self.call(|reply| Request::Notes(query, reply)).await
    }

    /// One note, in full. Fails with [`Error::NotFound`] if it was deleted.
    pub async fn note(&self, id: NoteId) -> Result<Note> {
        self.call(|reply| Request::Note(id, reply)).await
    }

    pub async fn add_note(&self, draft: NoteDraft) -> Result<Note> {
        self.call(|reply| Request::AddNote(draft, reply)).await
    }

    /// Saves a note's title and body, and returns the note as stored.
    pub async fn update_note_text(
        &self,
        id: NoteId,
        title: impl Into<String>,
        body: impl Into<String>,
    ) -> Result<Note> {
        let (title, body) = (title.into(), body.into());
        self.call(|reply| Request::UpdateNoteText(id, title, body, reply))
            .await
    }

    /// Replaces the children a note is about.
    pub async fn set_note_children(&self, id: NoteId, children: Vec<ChildId>) -> Result<Note> {
        self.call(|reply| Request::SetNoteChildren(id, children, reply))
            .await
    }

    /// Replaces a note's labels.
    pub async fn set_note_labels(&self, id: NoteId, labels: Vec<LabelId>) -> Result<Note> {
        self.call(|reply| Request::SetNoteLabels(id, labels, reply))
            .await
    }

    /// Shares the note with a child, or stops. Fails with
    /// [`Error::CantShareNote`] unless the note is about the child and
    /// sharing with the child is on.
    pub async fn set_note_shared(&self, id: NoteId, child: ChildId, shared: bool) -> Result<Note> {
        self.call(|reply| Request::SetNoteShared(id, child, shared, reply))
            .await
    }

    /// Deletes a note, and its images with it.
    pub async fn delete_note(&self, id: NoteId) -> Result<()> {
        self.call(|reply| Request::DeleteNote(id, reply)).await
    }

    /// A note's images in the order they were added, with thumbnails.
    pub async fn note_images(&self, note: NoteId) -> Result<Vec<NoteImage>> {
        self.call(|reply| Request::NoteImages(note, reply)).await
    }

    /// Attaches an image made by [`images::prepare`](crate::images::prepare).
    /// If the note already has the same image, returns that one instead.
    pub async fn add_image(&self, note: NoteId, image: PreparedImage) -> Result<NoteImage> {
        let image = Box::new(image);
        self.call(|reply| Request::AddImage(note, image, reply))
            .await
    }

    /// The image to show full size: at most
    /// [`DISPLAY_EDGE`](crate::images::DISPLAY_EDGE) pixels, upright.
    pub async fn display_image(&self, id: ImageId) -> Result<EncodedImage> {
        self.call(|reply| Request::DisplayImage(id, reply)).await
    }

    /// Deletes an image and its bytes.
    pub async fn delete_image(&self, id: ImageId) -> Result<()> {
        self.call(|reply| Request::DeleteImage(id, reply)).await
    }

    // --- Sharing. The `Sharing` actor uses most of these; the UI reads. ---

    /// The father's relay account, if he's signed in.
    pub async fn account(&self) -> Result<Option<Account>> {
        self.call(Request::Account).await
    }

    pub(crate) async fn set_account(&self, account: Account) -> Result<()> {
        self.call(|reply| Request::SetAccount(account, reply)).await
    }

    pub(crate) async fn clear_account(&self) -> Result<()> {
        self.call(Request::ClearAccount).await
    }

    /// Every child's share, in sidebar order.
    pub async fn shares(&self) -> Result<Vec<ChildShare>> {
        self.call(Request::Shares).await
    }

    pub async fn share(&self, child: ChildId) -> Result<Option<ChildShare>> {
        self.call(|reply| Request::Share(child, reply)).await
    }

    pub(crate) async fn add_share(&self, share: ChildShare, content_key: ContentKey) -> Result<()> {
        let share = Box::new(share);
        self.call(|reply| Request::AddShare(share, content_key, reply))
            .await
    }

    pub(crate) async fn rekey_share(&self, rekey: Rekey) -> Result<ChildShare> {
        let rekey = Box::new(rekey);
        self.call(|reply| Request::RekeyShare(rekey, reply)).await
    }

    pub(crate) async fn auth_key_updated(&self, child: ChildId, epoch: i64) -> Result<()> {
        self.call(|reply| Request::AuthKeyUpdated(child, epoch, reply))
            .await
    }

    pub(crate) async fn share_synced(&self, child: ChildId, at: jiff::Timestamp) -> Result<()> {
        self.call(|reply| Request::ShareSynced(child, at, reply))
            .await
    }

    pub(crate) async fn remove_share(&self, child: ChildId) -> Result<()> {
        self.call(|reply| Request::RemoveShare(child, reply)).await
    }

    /// See [`projection`](crate::share::projection).
    pub(crate) async fn share_changes(
        &self,
        child: ChildId,
        manifest: Manifest,
        budget: usize,
    ) -> Result<ShareChanges> {
        let manifest = Box::new(manifest);
        self.call(|reply| Request::ShareChanges(child, manifest, budget, reply))
            .await
    }

    /// Whether the vault is encrypted with a password.
    pub async fn is_encrypted(&self) -> Result<bool> {
        self.call(Request::IsEncrypted).await
    }

    /// Gives the vault a password, encrypting it, or changes the password
    /// it has. `current` must be the current password, if there is one;
    /// otherwise it fails with [`Error::WrongPassword`].
    ///
    /// Encrypting rewrites the whole vault, so it takes a moment for a
    /// large one. Changing the password only rewrites its key file.
    pub async fn set_password(&self, current: Option<Password>, new: Password) -> Result<()> {
        self.call(|reply| Request::SetPassword(current, new, reply))
            .await
    }

    /// Takes the password off the vault, decrypting it, after checking
    /// `current` is the password.
    pub async fn remove_password(&self, current: Password) -> Result<()> {
        self.call(|reply| Request::RemovePassword(current, reply))
            .await
    }

    /// Writes a backup of the vault to `dest` (see [`crate::backup`]).
    pub async fn backup(&self, dest: impl Into<PathBuf>) -> Result<()> {
        let dest = dest.into();
        self.call(|reply| Request::Backup(dest, reply)).await
    }

    /// Reads every row of every table: every page of an encrypted vault
    /// is decrypted on the way.
    pub(crate) async fn check(&self) -> Result<()> {
        self.call(Request::Check).await
    }

    /// Closes the vault and stops the database thread. Other clones of this
    /// `Store` get [`Error::StoreClosed`] from then on.
    ///
    /// Locking an encrypted vault calls this: dropping every clone would
    /// close it too, but only once the last one is gone, and a forgotten
    /// clone would keep the key in memory.
    pub async fn close(&self) -> Result<()> {
        self.call(Request::Close).await
    }

    /// Sends the request built by `request` and waits for the reply.
    async fn call<T>(&self, request: impl FnOnce(Reply<T>) -> Request) -> Result<T> {
        let (reply, response) = oneshot::channel();
        self.requests
            .send(request(reply))
            .map_err(|_| Error::StoreClosed)?;
        response.await.map_err(|_| Error::StoreClosed)?
    }
}

/// Finds the vault's key, then opens it. A plain vault left with a key file
/// by an interrupted change of password gets rid of it.
async fn open(location: &Location, unlock: Unlock) -> Result<Db> {
    let path = location.path();
    let key = match unlock {
        Unlock::Key(key) => Some(key),
        Unlock::Password(password) => {
            let path = path.ok_or(Error::InMemoryVault)?;
            Some(KeyFile::read(&key_path(path))?.unwrap(&password)?)
        }
        Unlock::Plain => {
            if let Some(path) = path
                && lock::inspect(path)? == VaultKind::Encrypted
            {
                return Err(Error::Locked);
            }
            None
        }
    };
    let plain = key.is_none();
    let db = Db::open(location, key).await?;
    if plain && let Some(path) = path {
        crate::files::remove_if_exists(&key_path(path))?;
    }
    Ok(db)
}

/// The body of the database thread.
fn serve(
    location: Location,
    unlock: Unlock,
    mut inbox: mpsc::UnboundedReceiver<Request>,
    opened: oneshot::Sender<Result<()>>,
) {
    // A single-threaded runtime: this thread is the only one that touches the
    // database. tokio is here for its channels now, and for turso's sync
    // (which needs it) in later phases.
    let runtime = match tokio::runtime::Builder::new_current_thread().build() {
        Ok(runtime) => runtime,
        Err(err) => {
            let _ = opened.send(Err(Error::StartThread(err)));
            return;
        }
    };

    runtime.block_on(async move {
        let mut db = match open(&location, unlock).await {
            Ok(db) => db,
            Err(err) => {
                let _ = opened.send(Err(err));
                return;
            }
        };
        let _ = opened.send(Ok(()));

        // `recv` returns `None` once every `Store` clone has been dropped.
        while let Some(request) = inbox.recv().await {
            // Requests that replace or close the database take it by value;
            // the rest borrow it.
            let (next, reply, result) = match request {
                Request::SetPassword(current, new, reply) => {
                    let (next, result) = rekey::set_password(db, current, new).await;
                    (next, reply, result)
                }
                Request::RemovePassword(current, reply) => {
                    let (next, result) = rekey::remove_password(db, current).await;
                    (next, reply, result)
                }
                Request::Close(reply) => (None, reply, db.close().await),
                request => {
                    handle(&mut db, request).await;
                    continue;
                }
            };
            let _ = reply.send(result);
            match next {
                Some(next) => db = next,
                // Closed, or couldn't be reopened: requests still queued
                // get `StoreClosed` when the inbox drops.
                None => return,
            }
        }
    });
}

async fn handle(db: &mut Db, request: Request) {
    // `send` fails only if the caller stopped waiting (say, its window
    // closed). The work is done either way, so the error is ignored.
    match request {
        Request::Children(reply) => {
            let _ = reply.send(db.children().await);
        }
        Request::AddChild(draft, reply) => {
            let _ = reply.send(db.add_child(draft).await);
        }
        Request::UpdateChild(id, draft, reply) => {
            let _ = reply.send(db.update_child(id, draft).await);
        }
        Request::DeleteChild(id, reply) => {
            let _ = reply.send(db.delete_child(id).await);
        }
        Request::Labels(reply) => {
            let _ = reply.send(db.labels().await);
        }
        Request::AddLabel(name, reply) => {
            let _ = reply.send(db.add_label(&name).await);
        }
        Request::RenameLabel(id, name, reply) => {
            let _ = reply.send(db.rename_label(id, &name).await);
        }
        Request::DeleteLabel(id, reply) => {
            let _ = reply.send(db.delete_label(id).await);
        }
        Request::Notes(query, reply) => {
            let _ = reply.send(db.notes(&query).await);
        }
        Request::Note(id, reply) => {
            let _ = reply.send(db.note(id).await);
        }
        Request::AddNote(draft, reply) => {
            let _ = reply.send(db.add_note(draft).await);
        }
        Request::UpdateNoteText(id, title, body, reply) => {
            let _ = reply.send(db.update_note_text(id, &title, &body).await);
        }
        Request::SetNoteChildren(id, children, reply) => {
            let _ = reply.send(db.set_note_children(id, &children).await);
        }
        Request::SetNoteLabels(id, labels, reply) => {
            let _ = reply.send(db.set_note_labels(id, &labels).await);
        }
        Request::SetNoteShared(id, child, shared, reply) => {
            let _ = reply.send(db.set_note_shared(id, child, shared).await);
        }
        Request::Account(reply) => {
            let _ = reply.send(db.account().await);
        }
        Request::SetAccount(account, reply) => {
            let _ = reply.send(db.set_account(&account).await);
        }
        Request::ClearAccount(reply) => {
            let _ = reply.send(db.clear_account().await);
        }
        Request::Shares(reply) => {
            let _ = reply.send(db.shares().await);
        }
        Request::Share(child, reply) => {
            let _ = reply.send(db.share(child).await);
        }
        Request::AddShare(share, content_key, reply) => {
            let _ = reply.send(db.add_share(&share, &content_key).await);
        }
        Request::RekeyShare(rekey, reply) => {
            let Rekey {
                child,
                epoch,
                access_key,
                content_key,
                wrapped,
            } = *rekey;
            let result = db
                .rekey_share(child, epoch, &access_key, &content_key, &wrapped)
                .await;
            let _ = reply.send(result);
        }
        Request::AuthKeyUpdated(child, epoch, reply) => {
            let _ = reply.send(db.auth_key_updated(child, epoch).await);
        }
        Request::ShareSynced(child, at, reply) => {
            let _ = reply.send(db.share_synced(child, at).await);
        }
        Request::RemoveShare(child, reply) => {
            let _ = reply.send(db.remove_share(child).await);
        }
        Request::ShareChanges(child, manifest, budget, reply) => {
            let _ = reply.send(db.share_changes(child, &manifest, budget).await);
        }
        Request::DeleteNote(id, reply) => {
            let _ = reply.send(db.delete_note(id).await);
        }
        Request::NoteImages(note, reply) => {
            let _ = reply.send(db.note_images(note).await);
        }
        Request::AddImage(note, image, reply) => {
            let _ = reply.send(db.add_image(note, *image).await);
        }
        Request::DisplayImage(id, reply) => {
            let _ = reply.send(db.display_image(id).await);
        }
        Request::DeleteImage(id, reply) => {
            let _ = reply.send(db.delete_image(id).await);
        }
        Request::IsEncrypted(reply) => {
            let _ = reply.send(Ok(db.key().is_some()));
        }
        Request::Backup(dest, reply) => {
            let _ = reply.send(db.backup(&dest).await);
        }
        Request::Check(reply) => {
            let _ = reply.send(db.check().await);
        }
        Request::SetPassword(..) | Request::RemovePassword(..) | Request::Close(..) => {
            unreachable!("handled by `serve`")
        }
    }
}

#[cfg(test)]
mod share_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod vault_tests;
