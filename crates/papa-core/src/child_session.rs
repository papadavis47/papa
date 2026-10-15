//! Sharing, the child's side: signing in with Dad's email and an access
//! key, keeping a replica of the share, pulling what changed, decrypting
//! it, and writing to Dad.
//!
//! Like [`Sharing`](crate::Sharing), an actor on a thread of its own,
//! "papa-child": the relay client and Turso's sync need tokio's IO, and a
//! pull waits on the network.
//!
//! ```text
//!  UI ──▶ ChildSession ──┬─▶ RelayClient: a token for the share
//!                        └─▶ Replica (child/share.db) ◀──pull── cloud
//!                              │ ciphertext
//!                              ▼ enc_key → content key → each row
//!                            SharedNote, in memory only
//! ```
//!
//! **Offline caching.** The replica is the cache: whatever the last pull
//! brought stays on disk, as ciphertext, and opening it needs no network.
//! So the child can read Dad's notes on a plane; a pull later brings what's
//! new. The sign-in is kept next to it (`session.json`, readable only by
//! the child's user account), so the app opens straight to the notes.
//!
//! **Messages.** The child's app reads the share's rows, which are the
//! father's, and adds two kinds of its own: messages to Dad, and receipts
//! for his. Both are new rows, never changes, so they can't collide with
//! anything Dad's app writes. A message goes into the replica first, then
//! out with a push; if that fails (offline, say), it waits there, and the
//! next pull pushes it before pulling. Until then it's in the outbox
//! (`outbox.json`), so the app can show it as "Sending…", even after a
//! restart.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;

use jiff::Timestamp;
use papa_proto::Secret;
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot};

use crate::model::clean_message;
use crate::relay::RelayClient;
use crate::share::projection::{BlobKind, SealedNote, decode_image};
use crate::share::replica::{self, MESSAGES_VERSION, Replica, TokenCell};
use crate::share::{
    Aad, AccessKey, ChildKeys, ContentKey, EncKey, KeyCard, NotePayload, Sealed, SealedMessage,
};
use crate::{
    EncodedImage, Error, ImageId, Message, MessageId, NoteId, NoteImage, Result, Sender, SharedNote,
};

/// A database token is renewed this long before it expires, so it can't
/// run out halfway through a pull.
const TOKEN_MARGIN_MS: i64 = 5 * 60 * 1000;

/// How long a token is used before the relay is asked for another, even
/// if it's good for longer. Asking is how a child's app learns its key
/// stopped working: Dad stopped sharing, or made a new key. (On Turso
/// Cloud a stopped share also fails the pull at once: its database is
/// gone.)
const TOKEN_REUSE_MS: i64 = 15 * 60 * 1000;

/// The sign-in, in the child folder.
const SESSION_FILE: &str = "session.json";
/// The replica of the share database, in the child folder.
const REPLICA_FILE: &str = "share.db";
/// The child's messages not pushed yet, in the child folder.
const OUTBOX_FILE: &str = "outbox.json";

/// What the child is signed in to, and what's in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShareContents {
    pub father_email: String,
    pub relay_url: String,
    /// Newest first.
    pub notes: Vec<SharedNote>,
    /// The conversation with Dad, oldest first.
    pub messages: Vec<Message>,
    /// The child can write to Dad: his Papa has made the share's message
    /// tables, and its key is this one.
    pub can_message: bool,
    /// The share is still sealed under a key this one doesn't open: Dad
    /// made a new key, and his app hasn't synced since. The relay took the
    /// key, so it's the right one; the notes come with his next sync.
    pub waiting: bool,
}

/// A handle to the child's thread. Cheap to clone. The thread stops when
/// the last clone is dropped.
#[derive(Clone, Debug)]
pub struct ChildSession {
    requests: mpsc::UnboundedSender<Request>,
}

type Reply<T> = oneshot::Sender<Result<T>>;

enum Request {
    Open(Reply<Option<ShareContents>>),
    SignIn {
        father_email: String,
        key: KeyCard,
        reply: Reply<ShareContents>,
    },
    Pull(Reply<Option<ShareContents>>),
    Send(MessageId, String, Reply<Message>),
    MarkRead(Reply<usize>),
    NoteImages(NoteId, Reply<Vec<NoteImage>>),
    Image(ImageId, BlobKind, Reply<EncodedImage>),
    SignOut(Reply<()>),
}

impl ChildSession {
    /// Starts the child's thread. Its files go in `dir`
    /// ([`paths::child_dir`](crate::paths::child_dir)).
    pub fn start(dir: PathBuf) -> Result<Self> {
        let (requests, inbox) = mpsc::unbounded_channel();
        let worker = Worker {
            dir,
            http: None,
            open: None,
        };
        thread::Builder::new()
            .name("papa-child".into())
            .spawn(move || worker.run(inbox))
            .map_err(Error::StartThread)?;
        Ok(Self { requests })
    }

    /// Whether a child signed in here, so the app opens in child mode.
    /// Reads nothing but the folder.
    pub fn is_signed_in(dir: &Path) -> bool {
        dir.join(SESSION_FILE).is_file()
    }

    /// Opens the saved sign-in and reads the notes the last pull brought,
    /// without the network. `None` if nobody signed in here.
    pub async fn open(&self) -> Result<Option<ShareContents>> {
        self.call(Request::Open).await
    }

    /// Signs in with Dad's email and the key he handed over, downloads the
    /// share, and remembers the sign-in. Replaces whatever was here.
    pub async fn sign_in(&self, father_email: String, key: KeyCard) -> Result<ShareContents> {
        self.call(|reply| Request::SignIn {
            father_email,
            key,
            reply,
        })
        .await
    }

    /// Pushes what this app wrote, then fetches what changed. `Some`, with
    /// everything, if anything did.
    ///
    /// Fails with [`Error::Relay`] and
    /// [`InvalidCredentials`](papa_proto::ErrorCode::InvalidCredentials)
    /// once the key stops working: Dad made a new one, or stopped sharing.
    pub async fn pull(&self) -> Result<Option<ShareContents>> {
        self.call(Request::Pull).await
    }

    /// Writes to Dad, as `id`: the id the UI gave the message when it
    /// showed it. The message is stored, then pushed; offline, it's
    /// stored and comes back with `sent: false`, and the next
    /// [`pull`](Self::pull) sends it. Fails with
    /// [`Error::MessagesUnavailable`] until `can_message`.
    pub async fn send(&self, id: MessageId, body: String) -> Result<Message> {
        self.call(|reply| Request::Send(id, body, reply)).await
    }

    /// The child read Dad's messages: adds a receipt for each that has
    /// none, and pushes them if it can. Returns how many.
    pub async fn mark_read(&self) -> Result<usize> {
        self.call(Request::MarkRead).await
    }

    /// A note's images: thumbnails, and the size of each display copy.
    pub async fn note_images(&self, note: NoteId) -> Result<Vec<NoteImage>> {
        self.call(|reply| Request::NoteImages(note, reply)).await
    }

    /// One version of an image. The display copy is the largest there is:
    /// originals stay on Dad's computer.
    pub async fn image(&self, image: ImageId, kind: BlobKind) -> Result<EncodedImage> {
        self.call(|reply| Request::Image(image, kind, reply)).await
    }

    /// Forgets the sign-in and deletes the replica. Signing in again
    /// downloads everything anew.
    pub async fn sign_out(&self) -> Result<()> {
        self.call(Request::SignOut).await
    }

    async fn call<T>(&self, request: impl FnOnce(Reply<T>) -> Request) -> Result<T> {
        let (reply, response) = oneshot::channel();
        self.requests
            .send(request(reply))
            .map_err(|_| Error::StoreClosed)?;
        response.await.map_err(|_| Error::StoreClosed)?
    }
}

/// The sign-in as `session.json` keeps it. The access key is in it: the
/// app needs it to decrypt, at every start, without asking. The file is
/// readable only by the child's user account, like the father's key file.
#[derive(Serialize, Deserialize)]
struct SavedSession {
    relay_url: String,
    father_email: String,
    share_id: String,
    database_url: String,
    access_key: Secret,
}

/// An open share: the replica, the keys that read it, and its token.
struct OpenShare {
    saved: SavedSession,
    keys: ChildKeys,
    replica: Replica,
    token: TokenCell,
    /// When to ask the relay for a new token, in Unix milliseconds: well
    /// before it expires, and every [`TOKEN_REUSE_MS`]. 0 until one is
    /// fetched: opening offline needs none.
    renew_at: i64,
    /// The content key of the share's current epoch, once there is one.
    content: Option<(i64, ContentKey)>,
    /// Each note's images, from the last read: what the gallery asks for.
    images: HashMap<NoteId, Vec<ImageId>>,
    /// The child's messages that may not have been pushed yet.
    outbox: HashSet<MessageId>,
}

/// The child thread's state. Only that thread touches it.
struct Worker {
    dir: PathBuf,
    /// Made on first use: building one loads the system's certificates.
    http: Option<reqwest::Client>,
    open: Option<OpenShare>,
}

impl Worker {
    fn run(mut self, mut inbox: mpsc::UnboundedReceiver<Request>) {
        let runtime = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            // Requests get `StoreClosed` when the inbox drops.
            Err(_) => return,
        };
        runtime.block_on(async move {
            while let Some(request) = inbox.recv().await {
                self.handle(request).await;
            }
        });
    }

    async fn handle(&mut self, request: Request) {
        // A failed `send` means the caller stopped waiting.
        match request {
            Request::Open(reply) => {
                let _ = reply.send(self.open().await);
            }
            Request::SignIn {
                father_email,
                key,
                reply,
            } => {
                let _ = reply.send(self.sign_in(father_email, key).await);
            }
            Request::Pull(reply) => {
                let _ = reply.send(self.pull().await);
            }
            Request::Send(id, body, reply) => {
                let _ = reply.send(self.send(id, body).await);
            }
            Request::MarkRead(reply) => {
                let _ = reply.send(self.mark_read().await);
            }
            Request::NoteImages(note, reply) => {
                let _ = reply.send(self.note_images(note).await);
            }
            Request::Image(image, kind, reply) => {
                let _ = reply.send(self.image(image, kind).await);
            }
            Request::SignOut(reply) => {
                let _ = reply.send(self.sign_out());
            }
        }
    }

    fn relay(&mut self, relay_url: &str) -> Result<RelayClient> {
        let http = match &self.http {
            Some(http) => http.clone(),
            None => self.http.insert(crate::relay::http_client()?).clone(),
        };
        Ok(RelayClient::with_client(http, relay_url))
    }

    fn replica_path(&self) -> PathBuf {
        self.dir.join(REPLICA_FILE)
    }

    fn session_path(&self) -> PathBuf {
        self.dir.join(SESSION_FILE)
    }

    fn outbox_path(&self) -> PathBuf {
        self.dir.join(OUTBOX_FILE)
    }

    async fn open(&mut self) -> Result<Option<ShareContents>> {
        if self.open.is_none() {
            let Some(saved) = self.load_session()? else {
                return Ok(None);
            };
            let access_key: AccessKey = saved.access_key.expose().parse()?;
            let keys = ChildKeys::derive(&access_key)?;
            let token: TokenCell = Arc::new(Mutex::new(Secret::new("")));
            let path = self.replica_path();
            let mut renew_at = 0;
            let fresh = !path.exists();
            if fresh {
                // Deleted since: download it again, which needs a token.
                let relay = self.relay(&saved.relay_url)?;
                let fresh = relay.child_token(&saved.father_email, &keys.auth).await?;
                *token.lock().expect("never poisoned") = fresh.database.token;
                renew_at = renew_at_for(fresh.database.expires_at, Timestamp::now());
            }
            let replica = Replica::open_as_child(&path, &saved.database_url, token.clone())
                .await
                .inspect_err(|_| {
                    // Never the cache that's there: only a half-made one.
                    if fresh {
                        let _ = replica::remove_files(&path);
                    }
                })?;
            let outbox = load_outbox(&self.outbox_path())?;
            self.open = Some(OpenShare {
                saved,
                keys,
                replica,
                token,
                renew_at,
                content: None,
                images: HashMap::new(),
                outbox,
            });
        }
        let open = self.open.as_mut().expect("opened just above");
        read(open).await.map(Some)
    }

    async fn sign_in(&mut self, father_email: String, key: KeyCard) -> Result<ShareContents> {
        let father_email = father_email.trim().to_owned();
        let keys = ChildKeys::derive(&key.access_key)?;
        let relay = self.relay(&key.relay_url)?;
        let token = relay.child_token(&father_email, &keys.auth).await?;

        // Whatever was here before goes: another child's share, or this
        // one under an old key. Dropping the replica closes it first.
        self.open = None;
        let path = self.replica_path();
        replica::remove_files(&path)?;
        crate::files::remove_if_exists(&self.outbox_path())?;
        crate::files::create_private_dir(&self.dir)?;

        let cell: TokenCell = Arc::new(Mutex::new(token.database.token));
        let saved = SavedSession {
            relay_url: key.relay_url.clone(),
            father_email: father_email.clone(),
            share_id: token.share_id,
            database_url: token.database.database_url,
            access_key: Secret::new(key.access_key.to_string()),
        };
        let opened = async {
            let replica = Replica::open_as_child(&path, &saved.database_url, cell.clone()).await?;
            let mut open = OpenShare {
                saved,
                keys,
                replica,
                token: cell,
                renew_at: renew_at_for(token.database.expires_at, Timestamp::now()),
                content: None,
                images: HashMap::new(),
                outbox: HashSet::new(),
            };
            let notes = read(&mut open).await?;
            Ok::<_, Error>((open, notes))
        }
        .await;
        let (open, notes) = match opened {
            Ok(opened) => opened,
            Err(err) => {
                let _ = replica::remove_files(&path);
                return Err(err);
            }
        };
        let json = serde_json::to_vec_pretty(&open.saved).expect("plain strings serialize");
        crate::files::write_atomic(&self.session_path(), &json)?;
        self.open = Some(open);
        Ok(notes)
    }

    async fn pull(&mut self) -> Result<Option<ShareContents>> {
        self.fresh_token().await?;
        let pulled = self.sync_once().await;
        // A token lasts an hour, and can outlive the key it was given for:
        // Dad may have made a new key since, or stopped sharing. Before
        // calling the share broken, or waiting for a key, ask the relay
        // whether this key still works. If it doesn't, that's the error.
        let doubtful = match &pulled {
            Err(Error::Sync(_)) => true,
            Ok(Some(contents)) => contents.waiting,
            _ => false,
        };
        if !doubtful {
            return pulled;
        }
        if let Some(open) = &mut self.open {
            open.renew_at = 0;
        }
        self.fresh_token().await?;
        match pulled {
            // The key is good, so maybe the token wasn't: once more.
            Err(Error::Sync(_)) => self.sync_once().await,
            pulled => pulled,
        }
    }

    /// Pushes what's waiting, then pulls.
    async fn sync_once(&mut self) -> Result<Option<ShareContents>> {
        let pushed = self.push_pending().await?;
        let open = self.open.as_mut().ok_or(Error::ChildSignedOut)?;
        if !open.replica.pull().await? && !pushed {
            return Ok(None);
        }
        read(open).await.map(Some)
    }

    /// Pushes the messages and receipts written here since the last push,
    /// if there are any, and empties the outbox. `true` if anything went.
    async fn push_pending(&mut self) -> Result<bool> {
        let open = self.open.as_ref().ok_or(Error::ChildSignedOut)?;
        let unpushed = open.replica.unpushed().await? > 0;
        if unpushed {
            self.fresh_token().await?;
            let open = self.open.as_ref().ok_or(Error::ChildSignedOut)?;
            open.replica.push().await?;
        }
        let outbox_path = self.outbox_path();
        let open = self.open.as_mut().ok_or(Error::ChildSignedOut)?;
        // Turso has nothing left to send, so neither has the outbox (an
        // app that quit right after a push may have left ids in it).
        let had_outbox = !open.outbox.is_empty();
        if had_outbox {
            open.outbox.clear();
            crate::files::remove_if_exists(&outbox_path)?;
        }
        Ok(unpushed || had_outbox)
    }

    async fn send(&mut self, id: MessageId, body: String) -> Result<Message> {
        let body = clean_message(&body)?;
        let outbox_path = self.outbox_path();
        let open = self.open.as_mut().ok_or(Error::ChildSignedOut)?;
        if open.replica.schema_version().await? < MESSAGES_VERSION {
            return Err(Error::MessagesUnavailable);
        }
        let (epoch, key) = open.content.as_ref().ok_or(Error::MessagesUnavailable)?;
        let mut message = Message::draft(id, Sender::Child, body);
        let sealed = SealedMessage::seal(
            key,
            &open.saved.share_id,
            id,
            Sender::Child,
            message.created_at.as_millisecond(),
            *epoch,
            &message.body,
        )?;
        // The outbox first. A crash between the two leaves an id with no
        // message, which nothing shows; the other way round, a message
        // that looks sent and isn't.
        open.outbox.insert(id);
        save_outbox(&outbox_path, &open.outbox)?;
        open.replica.add_message(&sealed).await?;
        // Offline is fine: the message waits, and the next pull sends it.
        message.sent = self.push_pending().await.is_ok();
        Ok(message)
    }

    async fn mark_read(&mut self) -> Result<usize> {
        let open = self.open.as_mut().ok_or(Error::ChildSignedOut)?;
        let Some((epoch, _)) = open.content else {
            return Ok(0);
        };
        if open.replica.schema_version().await? < MESSAGES_VERSION {
            return Ok(0);
        }
        let read: HashSet<MessageId> = open
            .replica
            .receipts()
            .await?
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        let now = Timestamp::now().as_millisecond();
        // Only what the child can see: Dad's messages under this key.
        let receipts: Vec<(MessageId, i64)> = open
            .replica
            .sealed_messages(Some(Sender::Father))
            .await?
            .into_iter()
            .filter(|message| message.epoch == epoch && !read.contains(&message.id))
            .map(|message| (message.id, now))
            .collect();
        if receipts.is_empty() {
            return Ok(0);
        }
        open.replica.add_receipts(&receipts).await?;
        // Offline, the receipts wait for the next pull.
        let _ = self.push_pending().await;
        Ok(receipts.len())
    }

    /// Makes sure the open share's token is good for a while yet.
    async fn fresh_token(&mut self) -> Result<()> {
        let (relay_url, email, auth) = match &self.open {
            None => return Err(Error::ChildSignedOut),
            Some(open) if open.renew_at > Timestamp::now().as_millisecond() => {
                return Ok(());
            }
            Some(open) => (
                open.saved.relay_url.clone(),
                open.saved.father_email.clone(),
                open.keys.auth.clone(),
            ),
        };
        let token = self.relay(&relay_url)?.child_token(&email, &auth).await?;
        let open = self.open.as_mut().ok_or(Error::ChildSignedOut)?;
        *open.token.lock().expect("never poisoned") = token.database.token;
        open.renew_at = renew_at_for(token.database.expires_at, Timestamp::now());
        Ok(())
    }

    async fn note_images(&mut self, note: NoteId) -> Result<Vec<NoteImage>> {
        let ids = self
            .open
            .as_ref()
            .ok_or(Error::ChildSignedOut)?
            .images
            .get(&note)
            .cloned()
            .unwrap_or_default();
        let mut images = Vec::with_capacity(ids.len());
        for id in ids {
            let thumb = self.image(id, BlobKind::Thumb).await?;
            let display = self.image(id, BlobKind::Display).await?;
            let (width, height) = crate::images::dimensions(&display)?;
            images.push(NoteImage {
                id,
                note_id: note,
                format: display.format,
                width,
                height,
                thumb,
            });
        }
        Ok(images)
    }

    async fn image(&mut self, image: ImageId, kind: BlobKind) -> Result<EncodedImage> {
        let open = self.open.as_ref().ok_or(Error::ChildSignedOut)?;
        let (epoch, key) = open.content.as_ref().ok_or(Error::NotFound("image"))?;
        // Under another epoch, it's waiting for Dad's app to seal it again
        // with the new key: not readable yet.
        let sealed = match open.replica.sealed_blob(image, kind).await? {
            Some((row_epoch, sealed)) if row_epoch == *epoch => sealed,
            _ => return Err(Error::NotFound("image")),
        };
        let image_id = image.to_string();
        let aad = Aad::Blob {
            share: &open.saved.share_id,
            image: &image_id,
            kind: kind.as_str(),
            epoch: *epoch,
        };
        decode_image(key.open(aad, &sealed, "an image")?)
    }

    fn sign_out(&mut self) -> Result<()> {
        // Dropping the replica closes it before its files go.
        self.open = None;
        crate::files::remove_if_exists(&self.session_path())?;
        crate::files::remove_if_exists(&self.outbox_path())?;
        replica::remove_files(&self.replica_path())
    }

    fn load_session(&self) -> Result<Option<SavedSession>> {
        let path = self.session_path();
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(reason) => return Err(Error::ReadFile { path, reason }),
        };
        serde_json::from_str(&text)
            .map(Some)
            .map_err(|reason| Error::InvalidSettings { path, reason })
    }
}

/// The outbox at `path`: empty if there's no file.
fn load_outbox(path: &Path) -> Result<HashSet<MessageId>> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(HashSet::new()),
        Err(reason) => {
            return Err(Error::ReadFile {
                path: path.to_owned(),
                reason,
            });
        }
    };
    let ids: Vec<String> =
        serde_json::from_str(&text).map_err(|reason| Error::InvalidSettings {
            path: path.to_owned(),
            reason,
        })?;
    // An id that doesn't parse can't be any message's: nothing to show.
    Ok(ids.iter().filter_map(|id| id.parse().ok()).collect())
}

fn save_outbox(path: &Path, outbox: &HashSet<MessageId>) -> Result<()> {
    let ids: Vec<String> = outbox.iter().map(MessageId::to_string).collect();
    let json = serde_json::to_vec(&ids).expect("strings serialize");
    crate::files::write_atomic(path, &json)
}

/// When to renew a token, fetched `now`, that expires at `expires_at`.
fn renew_at_for(expires_at: i64, now: Timestamp) -> i64 {
    let reuse_until = now.as_millisecond() + TOKEN_REUSE_MS;
    (expires_at - TOKEN_MARGIN_MS).min(reuse_until)
}

/// Reads and decrypts every note and message in `open`'s replica, and
/// remembers the content key and each note's images for later requests.
async fn read(open: &mut OpenShare) -> Result<ShareContents> {
    // Until Dad's first sync, the database has no tables: nothing shared.
    let opened = if open.replica.readable().await? {
        let keys = open.replica.share_keys().await?;
        let rows = open.replica.sealed_notes().await?;
        open_notes(&open.saved.share_id, &open.keys.enc, keys, rows)?
    } else {
        Opened::Nothing
    };
    let mut contents = ShareContents {
        father_email: open.saved.father_email.clone(),
        relay_url: open.saved.relay_url.clone(),
        notes: Vec::new(),
        messages: Vec::new(),
        can_message: false,
        waiting: false,
    };
    open.content = None;
    open.images.clear();
    match opened {
        Opened::Nothing => {}
        Opened::Waiting => contents.waiting = true,
        Opened::Notes {
            epoch,
            content,
            notes,
        } => {
            open.images = notes
                .iter()
                .map(|note| (note.id, note.images.clone()))
                .collect();
            if open.replica.schema_version().await? >= MESSAGES_VERSION {
                let rows = open.replica.sealed_messages(None).await?;
                let receipts = open.replica.receipts().await?;
                contents.messages = open_messages(
                    &open.saved.share_id,
                    epoch,
                    &content,
                    rows,
                    receipts,
                    &open.outbox,
                )?;
                contents.can_message = true;
            }
            open.content = Some((epoch, content));
            contents.notes = notes;
        }
    }
    Ok(contents)
}

/// Decrypts the conversation: the messages sealed under the current
/// epoch, oldest first. Like notes, older ones wait for Dad's app to seal
/// them again, and a current one that won't open is an error.
fn open_messages(
    share: &str,
    epoch: i64,
    content: &ContentKey,
    rows: Vec<SealedMessage>,
    receipts: Vec<(MessageId, i64)>,
    outbox: &HashSet<MessageId>,
) -> Result<Vec<Message>> {
    let receipts: HashMap<MessageId, i64> = receipts.into_iter().collect();
    let mut messages = Vec::with_capacity(rows.len());
    for row in rows.into_iter().filter(|row| row.epoch == epoch) {
        let read_at = receipts
            .get(&row.id)
            .and_then(|ms| Timestamp::from_millisecond(*ms).ok());
        messages.push(Message {
            body: row.open(content, share)?,
            created_at: Timestamp::from_millisecond(row.created_at).map_err(|_| {
                Error::Corrupt {
                    what: "message date",
                    value: row.created_at.to_string(),
                }
            })?,
            sent: row.sender == Sender::Father || !outbox.contains(&row.id),
            read_at,
            ..Message::draft(row.id, row.sender, String::new())
        });
    }
    messages.sort_by_key(|message| (message.created_at, message.id));
    Ok(messages)
}

/// What a share's rows open to.
enum Opened {
    /// No key yet: nothing has been shared.
    Nothing,
    /// The key is for another access key (see [`ShareContents::waiting`]).
    Waiting,
    Notes {
        epoch: i64,
        content: ContentKey,
        /// Newest first.
        notes: Vec<SharedNote>,
    },
}

/// Decrypts a share's notes: unwraps the newest content key with `enc`,
/// then opens each note sealed under it.
///
/// Rows sealed under an older epoch are skipped: after a new key, Dad's
/// app seals everything again, and until it has, those rows belong to a
/// key this child no longer has. A row of the current epoch that won't
/// open is an error: it was changed, or moved to another row's place.
fn open_notes(
    share: &str,
    enc: &EncKey,
    keys: Vec<(i64, Sealed)>,
    rows: Vec<SealedNote>,
) -> Result<Opened> {
    let Some((epoch, wrapped)) = keys.into_iter().max_by_key(|(epoch, _)| *epoch) else {
        return Ok(Opened::Nothing);
    };
    let Ok(content) = ContentKey::unwrap(&wrapped, enc, share, epoch) else {
        return Ok(Opened::Waiting);
    };
    let mut notes = Vec::with_capacity(rows.len());
    for row in rows.into_iter().filter(|row| row.version.epoch == epoch) {
        let id = row.id.to_string();
        let aad = Aad::Note {
            share,
            note: &id,
            epoch,
        };
        let json = content.open(aad, &row.sealed, "a note")?;
        let payload: NotePayload = serde_json::from_slice(&json).map_err(|err| Error::Corrupt {
            what: "shared note",
            value: err.to_string(),
        })?;
        notes.push(shared_note(row.id, payload)?);
    }
    notes.sort_by_key(|note| std::cmp::Reverse((note.created_at, note.id)));
    Ok(Opened::Notes {
        epoch,
        content,
        notes,
    })
}

fn shared_note(id: NoteId, payload: NotePayload) -> Result<SharedNote> {
    let time = |ms: i64| {
        Timestamp::from_millisecond(ms).map_err(|_| Error::Corrupt {
            what: "note date",
            value: ms.to_string(),
        })
    };
    let images = payload
        .images
        .iter()
        .map(|image| {
            image.parse().map_err(|_| Error::Corrupt {
                what: "image id",
                value: image.clone(),
            })
        })
        .collect::<Result<_>>()?;
    Ok(SharedNote {
        id,
        title: payload.title,
        body: payload.body,
        images,
        created_at: time(payload.created_at)?,
        updated_at: time(payload.updated_at)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::share::projection::NoteVersion;

    const SHARE: &str = "0199d3c4-0000-7000-8000-000000000001";

    /// A share as Dad's app would leave it: the content key of `epoch`
    /// wrapped under `keys`, and one note sealed per `(title, created_at,
    /// epoch)`.
    fn share(
        keys: &ChildKeys,
        content: &ContentKey,
        epoch: i64,
        notes: &[(&str, i64, i64)],
    ) -> (Vec<(i64, Sealed)>, Vec<SealedNote>) {
        let wrapped = content.wrap(&keys.enc, SHARE, epoch).unwrap();
        let rows = notes
            .iter()
            .map(|(title, created_at, row_epoch)| {
                let id = NoteId::new();
                let payload = NotePayload {
                    title: (*title).to_owned(),
                    body: format!("About {title}."),
                    created_at: *created_at,
                    updated_at: *created_at,
                    images: Vec::new(),
                };
                let note = id.to_string();
                let aad = Aad::Note {
                    share: SHARE,
                    note: &note,
                    epoch: *row_epoch,
                };
                SealedNote {
                    id,
                    version: NoteVersion {
                        updated_at: *created_at,
                        epoch: *row_epoch,
                    },
                    sealed: content
                        .seal(aad, &serde_json::to_vec(&payload).unwrap())
                        .unwrap(),
                }
            })
            .collect();
        (vec![(epoch, wrapped)], rows)
    }

    fn opened(opened: Result<Opened>) -> (i64, Vec<SharedNote>) {
        match opened {
            Ok(Opened::Notes { epoch, notes, .. }) => (epoch, notes),
            Ok(_) => panic!("nothing opened"),
            Err(err) => panic!("{err}"),
        }
    }

    fn keys() -> ChildKeys {
        ChildKeys::derive(&AccessKey::generate().unwrap()).unwrap()
    }

    #[test]
    fn notes_open_newest_first() {
        let keys = keys();
        let content = ContentKey::generate().unwrap();
        let (wrapped, rows) = share(
            &keys,
            &content,
            1,
            &[("Bike", 1_000, 1), ("Tooth", 3_000, 1), ("Play", 2_000, 1)],
        );

        let (epoch, notes) = opened(open_notes(SHARE, &keys.enc, wrapped, rows));

        assert_eq!(epoch, 1);
        let titles: Vec<_> = notes.iter().map(|note| note.title.as_str()).collect();
        assert_eq!(titles, ["Tooth", "Play", "Bike"]);
        assert_eq!(notes[0].body, "About Tooth.");
        assert_eq!(notes[0].created_at.as_millisecond(), 3_000);
    }

    #[test]
    fn rows_of_an_older_epoch_wait_to_be_resealed() {
        let keys = keys();
        let content = ContentKey::generate().unwrap();
        // Epoch 2's key; one note still sealed under epoch 1.
        let (wrapped, rows) = share(&keys, &content, 2, &[("New", 2, 2), ("Old", 1, 1)]);

        let (_, notes) = opened(open_notes(SHARE, &keys.enc, wrapped, rows));

        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].title, "New");
    }

    #[test]
    fn a_key_for_another_access_key_waits_for_dads_next_sync() {
        let old = keys();
        let content = ContentKey::generate().unwrap();
        let (wrapped, rows) = share(&old, &content, 1, &[("Bike", 1, 1)]);

        let new = keys();
        assert!(matches!(
            open_notes(SHARE, &new.enc, wrapped, rows),
            Ok(Opened::Waiting)
        ));
    }

    #[test]
    fn a_moved_row_is_refused() {
        let keys = keys();
        let content = ContentKey::generate().unwrap();
        let (wrapped, rows) = share(&keys, &content, 1, &[("Bike", 1, 1), ("Play", 2, 1)]);

        // The server swaps two rows' contents: neither opens in the other's
        // place.
        let mut swapped = rows;
        let first = swapped[0].sealed.clone();
        swapped[0].sealed = swapped[1].sealed.clone();
        swapped[1].sealed = first;
        assert!(matches!(
            open_notes(SHARE, &keys.enc, wrapped, swapped),
            Err(Error::Decrypt("a note"))
        ));
    }

    #[test]
    fn tokens_are_renewed_every_15_minutes_or_before_they_expire() {
        let now = Timestamp::from_second(1_000_000).unwrap();
        let ms = now.as_millisecond();
        let minutes = |n: i64| n * 60 * 1000;
        // The relay's tokens last an hour: ask again after 15 minutes,
        // to learn whether the key still works.
        assert_eq!(renew_at_for(ms + minutes(60), now), ms + minutes(15));
        // A token about to expire is renewed 5 minutes ahead.
        assert_eq!(renew_at_for(ms + minutes(10), now), ms + minutes(5));
    }

    #[test]
    fn a_share_without_a_key_has_nothing_to_read() {
        assert!(matches!(
            open_notes(SHARE, &keys().enc, Vec::new(), Vec::new()),
            Ok(Opened::Nothing)
        ));
    }
}
