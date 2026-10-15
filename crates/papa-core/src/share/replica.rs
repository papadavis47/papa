//! A share database: the local replica in `shares/<child>.db` on the
//! father's computer, or `child/share.db` on the child's, synced with the
//! cloud database the relay made for the share.
//!
//! Turso's sync keeps a log of what changed locally since the last push;
//! `push` sends it, `pull` fetches what others wrote. Only the father
//! writes notes; each message row is written by its sender, and each
//! receipt by the message's recipient. So the two never write the same
//! row and last-push-wins never has to pick a winner. (After a new key
//! the father seals the child's messages again; the child never changes
//! a row once written.)
//!
//! The schema (PLAN.md §4, as built):
//!
//! | Table | Holds | Visible to the cloud |
//! |---|---|---|
//! | `share_schema` | the schema version | the version |
//! | `share_key` | the content key, wrapped under the access key | epoch |
//! | `shared_note` | one sealed [`NotePayload`](super::NotePayload) per note | id, `updated_at`, epoch, size |
//! | `shared_blob` | sealed image versions | ids, kind, epoch, size |
//! | `message` | sealed messages, both ways | id, sender, time, epoch, size |
//! | `receipt` | which messages were read | message id, time read |

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use papa_proto::Secret;
use turso::{Connection, Row, Value};

use super::Sealed;
use super::message::{MessageVersion, SealedMessage};
use super::projection::{BlobKind, Manifest, NoteVersion, SealedNote, ShareChanges};
use crate::{Error, ImageId, MessageId, NoteId, Result, Sender};

/// Migration `n` (from 1) brings a share database from version `n - 1` to
/// `n`. Append only, as for the vault. The version lives in a table rather
/// than `PRAGMA user_version`, which is in the file header and doesn't sync.
const MIGRATIONS: &[&str] = &[
    "
    CREATE TABLE share_schema (version INTEGER NOT NULL) STRICT;

    -- The content key of each epoch, wrapped under the child's enc_key.
    -- The father keeps only the current epoch's row.
    CREATE TABLE share_key (
        epoch       INTEGER PRIMARY KEY NOT NULL,
        nonce       BLOB NOT NULL,
        wrapped_key BLOB NOT NULL
    ) STRICT;

    -- One row per shared note. `updated_at` is the note's in the father's
    -- vault: what the projection compares to know a row is current.
    CREATE TABLE shared_note (
        id         TEXT PRIMARY KEY NOT NULL,
        updated_at INTEGER NOT NULL,
        key_epoch  INTEGER NOT NULL,
        nonce      BLOB NOT NULL,
        ciphertext BLOB NOT NULL
    ) STRICT;

    -- One row per version of a shared note's image. Separate rows for the
    -- thumbnail and the display copy: Turso reads a whole row at a time.
    CREATE TABLE shared_blob (
        id         TEXT NOT NULL,
        kind       TEXT NOT NULL CHECK (kind IN ('thumb', 'display')),
        note_id    TEXT NOT NULL,
        key_epoch  INTEGER NOT NULL,
        nonce      BLOB NOT NULL,
        ciphertext BLOB NOT NULL,
        PRIMARY KEY (id, kind)
    ) STRICT;
",
    "
    -- Messages, both ways (Phase 13). Each row is written by its sender;
    -- the father's app seals them all again after a new key.
    CREATE TABLE message (
        id         TEXT PRIMARY KEY NOT NULL,
        sender     TEXT NOT NULL CHECK (sender IN ('father', 'child')),
        created_at INTEGER NOT NULL,
        key_epoch  INTEGER NOT NULL,
        nonce      BLOB NOT NULL,
        ciphertext BLOB NOT NULL
    ) STRICT;

    -- A message's recipient read it. Written once, by the recipient.
    CREATE TABLE receipt (
        message_id TEXT PRIMARY KEY NOT NULL,
        read_at    INTEGER NOT NULL
    ) STRICT;
",
];

/// The first schema version with messages.
pub(crate) const MESSAGES_VERSION: i64 = 2;

/// The schema version this build writes.
const LATEST: i64 = MIGRATIONS.len() as i64;

/// How long opening a replica or pushing may take. Turso's sync does its
/// network IO on a thread of its own; if that thread dies, its operations
/// would never finish, and a sync would wait forever.
const SYNC_TIMEOUT: Duration = Duration::from_secs(120);

/// The token the sync engine sends. Shared with its callback, which reads
/// it before every request, so a fresh token reaches an open replica.
pub(crate) type TokenCell = Arc<Mutex<Secret>>;

pub(crate) struct Replica {
    /// Pushes and pulls; owns the sync engine's IO thread.
    db: turso::sync::Database,
    conn: Connection,
}

impl Replica {
    /// Opens the replica at `path`, creating it from the cloud database at
    /// `url` if it doesn't exist yet, and brings its schema up to date.
    pub(crate) async fn open(path: &Path, url: &str, token: TokenCell) -> Result<Self> {
        let mut replica = Self::open_as_child(path, url, token).await?;
        replica.migrate().await?;
        Ok(replica)
    }

    /// Opens a child's replica: like [`open`](Self::open), but it leaves
    /// the schema alone. That's the father's to make; until his first
    /// sync, the database is empty. A child only ever adds messages and
    /// receipts.
    ///
    /// A replica that exists opens without the network, from what the
    /// last pull left: the child's notes, offline. Creating one needs the
    /// server, and a failed attempt can leave files behind (see
    /// [`remove_files`]).
    pub(crate) async fn open_as_child(path: &Path, url: &str, token: TokenCell) -> Result<Self> {
        crate::tls::init();
        let path = path
            .to_str()
            .ok_or_else(|| Error::NonUtf8Path(path.to_owned()))?;
        let build = turso::sync::Builder::new_remote(path)
            .with_remote_url(url)
            .with_client_name("papa")
            .with_auth_token_fn(move || {
                let token = token.lock().expect("never poisoned").expose().to_owned();
                async move { Ok(token) }
            })
            .build();
        let db = in_time(build).await?;
        let conn = in_time(db.connect()).await?;
        Ok(Self { db, conn })
    }

    async fn migrate(&mut self) -> Result<()> {
        let current = self.schema_version().await?;
        if current > LATEST {
            return Err(Error::SchemaTooNew {
                found: current,
                supported: LATEST,
            });
        }
        for (version, sql) in (1..=LATEST)
            .zip(MIGRATIONS)
            .skip_while(|(v, _)| *v <= current)
        {
            let tx = self.conn.transaction().await?;
            tx.execute_batch(sql).await?;
            tx.execute("INSERT INTO share_schema (version) VALUES (?1)", [version])
                .await?;
            tx.commit().await?;
        }
        Ok(())
    }

    /// The share database's schema version: 0 until the father's first
    /// sync makes the tables.
    pub(crate) async fn schema_version(&self) -> Result<i64> {
        let mut rows = self
            .conn
            .query(
                "SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = 'share_schema'",
                (),
            )
            .await?;
        if rows.next().await?.is_none() {
            return Ok(0);
        }
        drop(rows);
        let mut rows = self
            .conn
            .query("SELECT coalesce(max(version), 0) FROM share_schema", ())
            .await?;
        let row = rows
            .next()
            .await?
            .ok_or(turso::Error::QueryReturnedNoRows)?;
        Ok(row.get(0)?)
    }

    /// What the replica holds, without the content.
    pub(crate) async fn manifest(&self) -> Result<Manifest> {
        let mut manifest = Manifest::default();
        let mut rows = self
            .conn
            .query("SELECT id, updated_at, key_epoch FROM shared_note", ())
            .await?;
        while let Some(row) = rows.next().await? {
            let id: NoteId = parse(&row, 0, "note id")?;
            let version = NoteVersion {
                updated_at: row.get(1)?,
                epoch: row.get(2)?,
            };
            manifest.notes.insert(id, version);
        }
        drop(rows);
        let mut rows = self
            .conn
            .query("SELECT id, kind, key_epoch FROM shared_blob", ())
            .await?;
        while let Some(row) = rows.next().await? {
            let id: ImageId = parse(&row, 0, "image id")?;
            let kind: BlobKind = row.get::<String>(1)?.parse()?;
            manifest.blobs.insert((id, kind), row.get(2)?);
        }
        drop(rows);
        for message in self.sealed_messages(None).await? {
            let version = MessageVersion {
                sender: message.sender,
                epoch: message.epoch,
            };
            manifest.messages.insert(message.id, version);
        }
        manifest.receipts = self.receipts().await?.into_iter().collect();
        Ok(manifest)
    }

    /// Makes `epoch`'s wrapped key the only one, unless it already is.
    pub(crate) async fn set_share_key(&mut self, epoch: i64, wrapped: &Sealed) -> Result<()> {
        let mut rows = self
            .conn
            .query("SELECT epoch, nonce, wrapped_key FROM share_key", ())
            .await?;
        let mut current = Vec::new();
        while let Some(row) = rows.next().await? {
            current.push((row.get::<i64>(0)?, row.get_value(1)?, row.get_value(2)?));
        }
        drop(rows);
        let wanted = (
            epoch,
            Value::Blob(wrapped.nonce.to_vec()),
            Value::Blob(wrapped.ciphertext.clone()),
        );
        // Writing the same row again would still be a change to push.
        if current == [wanted.clone()] {
            return Ok(());
        }
        let tx = self.conn.transaction().await?;
        tx.execute("DELETE FROM share_key", ()).await?;
        tx.execute(
            "INSERT INTO share_key (epoch, nonce, wrapped_key) VALUES (?1, ?2, ?3)",
            (wanted.0, wanted.1, wanted.2),
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Writes `changes`, all or nothing.
    pub(crate) async fn apply(&mut self, changes: &ShareChanges) -> Result<()> {
        if changes.is_empty() {
            return Ok(());
        }
        let tx = self.conn.transaction().await?;
        for note in &changes.notes {
            tx.execute(
                "INSERT INTO shared_note (id, updated_at, key_epoch, nonce, ciphertext) \
                 VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT (id) DO UPDATE SET \
                 updated_at = excluded.updated_at, key_epoch = excluded.key_epoch, \
                 nonce = excluded.nonce, ciphertext = excluded.ciphertext",
                (
                    note.id.to_string(),
                    note.version.updated_at,
                    note.version.epoch,
                    note.sealed.nonce.as_slice(),
                    note.sealed.ciphertext.as_slice(),
                ),
            )
            .await?;
        }
        for id in &changes.removed_notes {
            tx.execute("DELETE FROM shared_note WHERE id = ?1", [id.to_string()])
                .await?;
        }
        for blob in &changes.blobs {
            tx.execute(
                "INSERT INTO shared_blob (id, kind, note_id, key_epoch, nonce, ciphertext) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6) ON CONFLICT (id, kind) DO UPDATE SET \
                 note_id = excluded.note_id, key_epoch = excluded.key_epoch, \
                 nonce = excluded.nonce, ciphertext = excluded.ciphertext",
                (
                    blob.image.to_string(),
                    blob.kind.as_str(),
                    blob.note.to_string(),
                    blob.epoch,
                    blob.sealed.nonce.as_slice(),
                    blob.sealed.ciphertext.as_slice(),
                ),
            )
            .await?;
        }
        for (id, kind) in &changes.removed_blobs {
            tx.execute(
                "DELETE FROM shared_blob WHERE id = ?1 AND kind = ?2",
                (id.to_string(), kind.as_str()),
            )
            .await?;
        }
        for message in &changes.messages {
            insert_message(&tx, message).await?;
        }
        for (id, read_at) in &changes.receipts {
            insert_receipt(&tx, *id, *read_at).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Sends every local change the cloud doesn't have yet. Nothing to send
    /// is fine, and cheap.
    pub(crate) async fn push(&self) -> Result<()> {
        in_time(self.db.push()).await
    }

    /// Fetches what changed in the cloud since the last pull. `true` if
    /// anything did.
    pub(crate) async fn pull(&self) -> Result<bool> {
        in_time(self.db.pull()).await
    }

    /// How many local changes the cloud doesn't have yet. Reads only the
    /// local file.
    pub(crate) async fn unpushed(&self) -> Result<i64> {
        Ok(in_time(self.db.stats()).await?.cdc_operations)
    }

    // --- Messages: both sides read them, both add to them. ---

    /// Every message row, still sealed: only `sender`'s, if given.
    pub(crate) async fn sealed_messages(
        &self,
        sender: Option<Sender>,
    ) -> Result<Vec<SealedMessage>> {
        let mut rows = self
            .conn
            .query(
                "SELECT id, sender, created_at, key_epoch, nonce, ciphertext FROM message \
                 WHERE ?1 IS NULL OR sender = ?1",
                [sender.map(Sender::as_str)],
            )
            .await?;
        let mut messages = Vec::new();
        while let Some(row) = rows.next().await? {
            messages.push(SealedMessage {
                id: parse(&row, 0, "message id")?,
                sender: row.get::<String>(1)?.parse()?,
                created_at: row.get(2)?,
                epoch: row.get(3)?,
                sealed: sealed(&row, 4)?,
            });
        }
        Ok(messages)
    }

    /// Every receipt: which message was read, and when.
    pub(crate) async fn receipts(&self) -> Result<Vec<(MessageId, i64)>> {
        let mut rows = self
            .conn
            .query("SELECT message_id, read_at FROM receipt", ())
            .await?;
        let mut receipts = Vec::new();
        while let Some(row) = rows.next().await? {
            receipts.push((parse(&row, 0, "message id")?, row.get(1)?));
        }
        Ok(receipts)
    }

    /// Adds a message the child wrote: a plain insert, since a child's
    /// token may add rows but not change them. The id is new, so nothing
    /// is there to change.
    pub(crate) async fn add_message(&self, message: &SealedMessage) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO message (id, sender, created_at, key_epoch, nonce, ciphertext) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                (
                    message.id.to_string(),
                    message.sender.as_str(),
                    message.created_at,
                    message.epoch,
                    message.sealed.nonce.as_slice(),
                    message.sealed.ciphertext.as_slice(),
                ),
            )
            .await?;
        Ok(())
    }

    /// Adds the child's receipts. One already there stays as it was.
    pub(crate) async fn add_receipts(&mut self, receipts: &[(MessageId, i64)]) -> Result<()> {
        let tx = self.conn.transaction().await?;
        for (id, read_at) in receipts {
            insert_receipt(&tx, *id, *read_at).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    // --- Reading, on the child's side. Call `readable` first: a newer
    // father's app may have changed the tables. ---

    /// Fails if the father's app wrote a schema this build can't read.
    /// `false` if he hasn't synced yet, so there's nothing to read.
    pub(crate) async fn readable(&self) -> Result<bool> {
        match self.schema_version().await? {
            0 => Ok(false),
            found if found > LATEST => Err(Error::SchemaTooNew {
                found,
                supported: LATEST,
            }),
            _ => Ok(true),
        }
    }

    /// The wrapped content keys, by epoch. The father keeps one.
    pub(crate) async fn share_keys(&self) -> Result<Vec<(i64, Sealed)>> {
        let mut rows = self
            .conn
            .query("SELECT epoch, nonce, wrapped_key FROM share_key", ())
            .await?;
        let mut keys = Vec::new();
        while let Some(row) = rows.next().await? {
            keys.push((row.get(0)?, sealed(&row, 1)?));
        }
        Ok(keys)
    }

    /// Every note row, still sealed.
    pub(crate) async fn sealed_notes(&self) -> Result<Vec<SealedNote>> {
        let mut rows = self
            .conn
            .query(
                "SELECT id, updated_at, key_epoch, nonce, ciphertext FROM shared_note",
                (),
            )
            .await?;
        let mut notes = Vec::new();
        while let Some(row) = rows.next().await? {
            notes.push(SealedNote {
                id: parse(&row, 0, "note id")?,
                version: NoteVersion {
                    updated_at: row.get(1)?,
                    epoch: row.get(2)?,
                },
                sealed: sealed(&row, 3)?,
            });
        }
        Ok(notes)
    }

    /// One version of an image, still sealed, with its key epoch.
    pub(crate) async fn sealed_blob(
        &self,
        image: ImageId,
        kind: BlobKind,
    ) -> Result<Option<(i64, Sealed)>> {
        let mut rows = self
            .conn
            .query(
                "SELECT key_epoch, nonce, ciphertext FROM shared_blob WHERE id = ?1 AND kind = ?2",
                (image.to_string(), kind.as_str()),
            )
            .await?;
        match rows.next().await? {
            Some(row) => Ok(Some((row.get(0)?, sealed(&row, 1)?))),
            None => Ok(None),
        }
    }
}

/// Writes a message row, or replaces it: with the same message sealed
/// again, under a new key.
async fn insert_message(conn: &Connection, message: &SealedMessage) -> Result<()> {
    conn.execute(
        "INSERT INTO message (id, sender, created_at, key_epoch, nonce, ciphertext) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6) ON CONFLICT (id) DO UPDATE SET \
         key_epoch = excluded.key_epoch, nonce = excluded.nonce, \
         ciphertext = excluded.ciphertext",
        (
            message.id.to_string(),
            message.sender.as_str(),
            message.created_at,
            message.epoch,
            message.sealed.nonce.as_slice(),
            message.sealed.ciphertext.as_slice(),
        ),
    )
    .await?;
    Ok(())
}

/// Writes a receipt, unless the message has one: a message is read once.
async fn insert_receipt(conn: &Connection, id: MessageId, read_at: i64) -> Result<()> {
    conn.execute(
        "INSERT INTO receipt (message_id, read_at) VALUES (?1, ?2) \
         ON CONFLICT (message_id) DO NOTHING",
        (id.to_string(), read_at),
    )
    .await?;
    Ok(())
}

/// The nonce and ciphertext in columns `idx` and `idx + 1`.
fn sealed(row: &Row, idx: usize) -> Result<Sealed> {
    let blob = |idx| match row.get_value(idx)? {
        Value::Blob(bytes) => Ok(bytes),
        other => Err(Error::Corrupt {
            what: "sealed value",
            value: format!("{other:?}"),
        }),
    };
    Sealed::from_parts(&blob(idx)?, blob(idx + 1)?)
}

/// The files of the replica at `path`: the database, its log, and the sync
/// engine's bookkeeping next to it.
pub(crate) fn remove_files(path: &Path) -> Result<()> {
    let Some(dir) = path.parent() else {
        return Ok(());
    };
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return Ok(());
    };
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(reason) => {
            return Err(Error::ReadFile {
                path: dir.to_owned(),
                reason,
            });
        }
    };
    for entry in entries.flatten() {
        let file = entry.file_name();
        // `<child>.db`, `<child>.db-wal`, `<child>.db-info`, …
        if file
            .to_str()
            .is_some_and(|file| file == name || file.starts_with(&format!("{name}-")))
        {
            crate::files::remove_if_exists(&entry.path())?;
        }
    }
    Ok(())
}

/// Runs a sync operation, giving up after [`SYNC_TIMEOUT`].
async fn in_time<T>(operation: impl Future<Output = turso::Result<T>>) -> Result<T> {
    match tokio::time::timeout(SYNC_TIMEOUT, operation).await {
        Ok(result) => result.map_err(sync_error),
        Err(_) => Err(Error::Sync("the server took too long to answer".into())),
    }
}

/// Turso's sync errors say what the server answered; that's the useful
/// part. They hold no tokens: those travel in a header.
fn sync_error(err: turso::Error) -> Error {
    Error::Sync(err.to_string())
}

fn parse<T: std::str::FromStr>(row: &Row, idx: usize, what: &'static str) -> Result<T> {
    let text: String = row.get(idx)?;
    text.parse()
        .map_err(|_| Error::Corrupt { what, value: text })
}
