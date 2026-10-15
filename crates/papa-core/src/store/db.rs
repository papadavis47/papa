//! The only module that writes SQL to the vault, with `shares.rs` for
//! sharing's tables. It runs on the database thread; see
//! [`Store`](super::Store) for how requests get here.

use std::collections::HashMap;
use std::fmt::Display;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use jiff::Timestamp;
use turso::{Builder, Connection, Database, EncryptionOpts, Row, Value};

use super::migrations;
use crate::files;
use crate::lock::{KeyFile, VAULT_CIPHER, VaultKey, key_path};
use crate::model::{SearchTerms, check_color, clean_child_name, clean_label_name, preview};
use crate::{
    Child, ChildDraft, ChildId, EncodedImage, Error, ImageFormat, ImageId, Label, LabelId, Note,
    NoteDraft, NoteFilter, NoteId, NoteImage, NoteQuery, NoteSummary, PreparedImage, Result,
};

/// Where the vault lives.
#[derive(Clone, Debug)]
pub(super) enum Location {
    File(PathBuf),
    /// Gone when the store closes. For tests and, later, demo mode.
    Memory,
}

impl Location {
    pub(super) fn path(&self) -> Option<&Path> {
        match self {
            Self::File(path) => Some(path),
            Self::Memory => None,
        }
    }
}

pub(super) struct Db {
    pub(super) conn: Connection,
    /// Kept alongside the connection so the database outlives it.
    _database: Database,
    location: Location,
    /// The key the vault is encrypted with; `None` for a plain vault.
    key: Option<VaultKey>,
}

impl Db {
    /// Opens (or creates) the vault and brings its schema up to date.
    /// `key` must be the vault's key if it's encrypted, and `None` if not.
    pub(super) async fn open(location: &Location, key: Option<VaultKey>) -> Result<Self> {
        if let Some(dir) = location.path().and_then(Path::parent) {
            files::create_private_dir(dir)?;
        }
        let database = build(location, key.as_ref()).await?;
        let mut conn = database.connect()?;
        // Off by default, as in SQLite, and set per connection. Without it
        // deleting a child would leave dangling `note_child` rows.
        conn.execute("PRAGMA foreign_keys = ON", ()).await?;
        migrations::run(&mut conn).await?;

        Ok(Self {
            conn,
            _database: database,
            location: location.clone(),
            key,
        })
    }

    pub(super) fn location(&self) -> &Location {
        &self.location
    }

    pub(super) fn key(&self) -> Option<&VaultKey> {
        self.key.as_ref()
    }

    /// Closes the vault, first moving everything from the write-ahead log
    /// into the vault file. Turso doesn't do that itself on close, and the
    /// file must be whole before it's copied or replaced.
    ///
    /// Dropping the database also drops Turso's copy of the key, which
    /// Turso zeroes.
    pub(super) async fn close(self) -> Result<()> {
        checkpoint(&self.conn).await
    }

    pub(super) fn connection(&self) -> &Connection {
        &self.conn
    }

    /// Writes a backup to `dest` (see [`crate::backup`]).
    pub(super) async fn backup(&self, dest: &Path) -> Result<()> {
        let path = self.location.path().ok_or(Error::InMemoryVault)?;
        let key = match self.key {
            Some(_) => Some(KeyFile::read(&key_path(path))?),
            None => None,
        };
        // The copy is consistent: this thread runs nothing else until it's
        // done, and after the checkpoint the file holds every commit.
        checkpoint(&self.conn).await?;
        crate::backup::write(path, key, migrations::LATEST, dest)
    }

    /// Reads every row of every table, so every page is read (and, in an
    /// encrypted vault, decrypted and authenticated).
    pub(super) async fn check(&self) -> Result<()> {
        for table in [
            "child",
            "label",
            "note",
            "note_child",
            "note_label",
            "image",
            "image_data",
            "account",
            "share",
            "note_share",
        ] {
            let mut rows = self
                .conn
                .query(&format!("SELECT * FROM {table}"), ())
                .await?;
            while rows.next().await?.is_some() {}
        }
        Ok(())
    }

    // --- Children ---

    pub(super) async fn children(&self) -> Result<Vec<Child>> {
        let mut rows = self
            .conn
            .query(
                "SELECT id, name, color FROM child ORDER BY sort_order, id",
                (),
            )
            .await?;
        let mut children = Vec::new();
        while let Some(row) = rows.next().await? {
            children.push(child_from_row(&row)?);
        }
        Ok(children)
    }

    pub(super) async fn add_child(&self, draft: ChildDraft) -> Result<Child> {
        let child = Child {
            id: ChildId::new(),
            name: clean_child_name(&draft.name)?,
            color: check_color(draft.color)?,
        };
        // New children go to the end of the sidebar.
        self.conn
            .execute(
                "INSERT INTO child (id, name, color, sort_order) \
                 VALUES (?1, ?2, ?3, (SELECT coalesce(max(sort_order), -1) + 1 FROM child))",
                (child.id.to_string(), child.name.as_str(), child.color),
            )
            .await?;
        Ok(child)
    }

    pub(super) async fn update_child(&self, id: ChildId, draft: ChildDraft) -> Result<Child> {
        let child = Child {
            id,
            name: clean_child_name(&draft.name)?,
            color: check_color(draft.color)?,
        };
        let changed = self
            .conn
            .execute(
                "UPDATE child SET name = ?2, color = ?3 WHERE id = ?1",
                (id.to_string(), child.name.as_str(), child.color),
            )
            .await?;
        found(changed, "child")?;
        Ok(child)
    }

    pub(super) async fn delete_child(&self, id: ChildId) -> Result<()> {
        // The foreign key would refuse too, but only with a generic error.
        let mut rows = self
            .conn
            .query(
                "SELECT child.name FROM share JOIN child ON child.id = share.child_id \
                 WHERE share.child_id = ?1",
                [id.to_string()],
            )
            .await?;
        if let Some(row) = rows.next().await? {
            return Err(Error::ChildIsShared(row.get(0)?));
        }
        drop(rows);
        let changed = self
            .conn
            .execute("DELETE FROM child WHERE id = ?1", [id.to_string()])
            .await?;
        found(changed, "child")
    }

    // --- Labels ---

    /// All labels, alphabetically, ignoring case.
    pub(super) async fn labels(&self) -> Result<Vec<Label>> {
        let mut rows = self
            .conn
            .query("SELECT id, name FROM label ORDER BY name, id", ())
            .await?;
        let mut labels = Vec::new();
        while let Some(row) = rows.next().await? {
            labels.push(Label {
                id: parse_id(&row, 0, "label id")?,
                name: row.get(1)?,
            });
        }
        Ok(labels)
    }

    pub(super) async fn add_label(&self, name: &str) -> Result<Label> {
        let label = Label {
            id: LabelId::new(),
            name: clean_label_name(name)?,
        };
        self.ensure_label_name_free(&label.name, None).await?;
        self.conn
            .execute(
                "INSERT INTO label (id, name) VALUES (?1, ?2)",
                (label.id.to_string(), label.name.as_str()),
            )
            .await?;
        Ok(label)
    }

    pub(super) async fn rename_label(&self, id: LabelId, name: &str) -> Result<Label> {
        let label = Label {
            id,
            name: clean_label_name(name)?,
        };
        self.ensure_label_name_free(&label.name, Some(id)).await?;
        let changed = self
            .conn
            .execute(
                "UPDATE label SET name = ?2 WHERE id = ?1",
                (id.to_string(), label.name.as_str()),
            )
            .await?;
        found(changed, "label")?;
        Ok(label)
    }

    pub(super) async fn delete_label(&self, id: LabelId) -> Result<()> {
        let changed = self
            .conn
            .execute("DELETE FROM label WHERE id = ?1", [id.to_string()])
            .await?;
        found(changed, "label")
    }

    // --- Notes ---
    //
    // Methods that write several statements take `&mut self`: a transaction
    // needs exclusive use of the connection. The actor owns the `Db`, so it
    // can always lend that out.

    /// The notes `query` asks for, newest first.
    ///
    /// The search runs here in Rust rather than in SQL (see [`SearchTerms`]).
    /// Every note of the filter is read either way: a substring search can't
    /// use an index.
    pub(super) async fn notes(&self, query: &NoteQuery) -> Result<Vec<NoteSummary>> {
        // Joining a link table keeps only one child's or one label's notes.
        let (links, linked) = match query.filter {
            NoteFilter::All => (None, None),
            NoteFilter::Child(id) => (Some(&CHILD_LINKS), Some(id.to_string())),
            NoteFilter::Label(id) => (Some(&LABEL_LINKS), Some(id.to_string())),
        };
        let join = links.map_or(String::new(), |links| {
            format!(
                "JOIN {table} link ON link.note_id = note.id AND link.{column} = ?1",
                table = links.table,
                column = links.column,
            )
        });
        let sql = format!(
            "SELECT note.id, note.title, note.body_md, note.created_at FROM note {join} \
             WHERE note.deleted_at IS NULL ORDER BY note.created_at DESC, note.id DESC"
        );

        let terms = SearchTerms::new(&query.search);
        let mut notes = Vec::new();
        let mut rows = self
            .conn
            .query(&sql, turso::params_from_iter(linked))
            .await?;
        while let Some(row) = rows.next().await? {
            let title: String = row.get(1)?;
            let body: String = row.get(2)?;
            if !terms.matches(&title, &body) {
                continue;
            }
            notes.push(NoteSummary {
                id: parse_id(&row, 0, "note id")?,
                preview: preview(&body),
                title,
                children: Vec::new(),
                labels: Vec::new(),
                images: 0,
                created_at: timestamp(&row, 3)?,
            });
        }
        // A connection runs one statement at a time: `rows` must be gone
        // before the next query, or turso reports the connection busy.
        drop(rows);

        let mut children = self.linked(&CHILD_LINKS, None).await?;
        let mut labels = self.linked(&LABEL_LINKS, None).await?;
        let mut images = self.image_counts().await?;
        for note in &mut notes {
            note.children = children.remove(&note.id).unwrap_or_default();
            note.labels = labels.remove(&note.id).unwrap_or_default();
            note.images = images.remove(&note.id).unwrap_or_default();
        }
        Ok(notes)
    }

    /// How many images each note has, for notes that have any.
    async fn image_counts(&self) -> Result<HashMap<NoteId, usize>> {
        let mut rows = self
            .conn
            .query("SELECT note_id, count(*) FROM image GROUP BY note_id", ())
            .await?;
        let mut counts = HashMap::new();
        while let Some(row) = rows.next().await? {
            let count: i64 = row.get(1)?;
            counts.insert(
                parse_id(&row, 0, "note id")?,
                usize::try_from(count).unwrap_or_default(),
            );
        }
        Ok(counts)
    }

    pub(super) async fn note(&self, id: NoteId) -> Result<Note> {
        let mut rows = self
            .conn
            .query(
                "SELECT title, body_md, created_at, updated_at FROM note \
                 WHERE id = ?1 AND deleted_at IS NULL",
                [id.to_string()],
            )
            .await?;
        let row = rows.next().await?.ok_or(Error::NotFound("note"))?;
        drop(rows);

        Ok(Note {
            id,
            title: row.get(0)?,
            body: row.get(1)?,
            children: self.linked_to(&CHILD_LINKS, id).await?,
            labels: self.linked_to(&LABEL_LINKS, id).await?,
            shared_with: self.linked_to(&SHARE_LINKS, id).await?,
            created_at: timestamp(&row, 2)?,
            updated_at: timestamp(&row, 3)?,
        })
    }

    pub(super) async fn add_note(&mut self, draft: NoteDraft) -> Result<Note> {
        let id = NoteId::new();
        let now = Timestamp::now().as_millisecond();
        // All or nothing: a failed link (say, to a child deleted meanwhile)
        // leaves no half-made note behind.
        let tx = self.conn.transaction().await?;
        tx.execute(
            "INSERT INTO note (id, title, body_md, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?4)",
            (id.to_string(), draft.title.trim(), draft.body.as_str(), now),
        )
        .await?;
        set_links(&tx, &CHILD_LINKS, id, &draft.children).await?;
        set_links(&tx, &LABEL_LINKS, id, &draft.labels).await?;
        tx.commit().await?;
        self.note(id).await
    }

    /// Saves a note's title and body. Autosave calls this while the father
    /// types.
    pub(super) async fn update_note_text(
        &self,
        id: NoteId,
        title: &str,
        body: &str,
    ) -> Result<Note> {
        let changed = self
            .conn
            .execute(
                "UPDATE note SET title = ?2, body_md = ?3, updated_at = ?4 \
                 WHERE id = ?1 AND deleted_at IS NULL",
                (
                    id.to_string(),
                    title.trim(),
                    body,
                    Timestamp::now().as_millisecond(),
                ),
            )
            .await?;
        found(changed, "note")?;
        self.note(id).await
    }

    /// Replaces the children a note is about. A child it's no longer about
    /// stops seeing it, if it was shared with them.
    pub(super) async fn set_note_children(
        &mut self,
        id: NoteId,
        children: &[ChildId],
    ) -> Result<Note> {
        self.relink(id, &CHILD_LINKS, children).await
    }

    /// Shares a note with a child, or stops sharing it. The note must be
    /// about the child, and sharing with the child must be on.
    pub(super) async fn set_note_shared(
        &mut self,
        id: NoteId,
        child: ChildId,
        shared: bool,
    ) -> Result<Note> {
        if shared {
            let mut rows = self
                .conn
                .query(
                    "SELECT 1 FROM note_child \
                     JOIN share ON share.child_id = note_child.child_id \
                     JOIN note ON note.id = note_child.note_id AND note.deleted_at IS NULL \
                     WHERE note_child.note_id = ?1 AND note_child.child_id = ?2",
                    (id.to_string(), child.to_string()),
                )
                .await?;
            let allowed = rows.next().await?.is_some();
            drop(rows);
            if !allowed {
                // Say "not found" for a note that's gone, as elsewhere.
                self.note(id).await?;
                return Err(Error::CantShareNote);
            }
            self.conn
                .execute(
                    "INSERT OR IGNORE INTO note_share (note_id, child_id, shared_at) \
                     VALUES (?1, ?2, ?3)",
                    (
                        id.to_string(),
                        child.to_string(),
                        Timestamp::now().as_millisecond(),
                    ),
                )
                .await?;
        } else {
            self.conn
                .execute(
                    "DELETE FROM note_share WHERE note_id = ?1 AND child_id = ?2",
                    (id.to_string(), child.to_string()),
                )
                .await?;
        }
        self.note(id).await
    }

    /// Replaces a note's labels.
    pub(super) async fn set_note_labels(&mut self, id: NoteId, labels: &[LabelId]) -> Result<Note> {
        self.relink(id, &LABEL_LINKS, labels).await
    }

    /// Wipes a note's text, links and images and marks it deleted. See the
    /// `note` table in the migration for why the row stays.
    pub(super) async fn delete_note(&mut self, id: NoteId) -> Result<()> {
        let tx = self.conn.transaction().await?;
        let changed = tx
            .execute(
                "UPDATE note SET title = '', body_md = '', updated_at = ?2, deleted_at = ?2 \
                 WHERE id = ?1 AND deleted_at IS NULL",
                (id.to_string(), Timestamp::now().as_millisecond()),
            )
            .await?;
        // Returning early drops `tx` without committing, which rolls back.
        found(changed, "note")?;
        set_links::<ChildId>(&tx, &CHILD_LINKS, id, &[]).await?;
        set_links::<LabelId>(&tx, &LABEL_LINKS, id, &[]).await?;
        tx.execute(
            "DELETE FROM note_share WHERE note_id = ?1",
            [id.to_string()],
        )
        .await?;
        // The tombstone stays, so the images' `ON DELETE CASCADE` doesn't
        // fire: delete them here. Their bytes cascade from the images.
        tx.execute("DELETE FROM image WHERE note_id = ?1", [id.to_string()])
            .await?;
        tx.commit().await?;
        Ok(())
    }

    // --- Images ---

    /// A note's images, in the order they were added, with thumbnails.
    pub(super) async fn note_images(&self, note: NoteId) -> Result<Vec<NoteImage>> {
        let mut rows = self
            .conn
            .query(
                &format!("{SELECT_IMAGES} WHERE image.note_id = ?1 ORDER BY image.id"),
                [note.to_string()],
            )
            .await?;
        let mut images = Vec::new();
        while let Some(row) = rows.next().await? {
            images.push(image_from_row(&row)?);
        }
        Ok(images)
    }

    /// Attaches `image` to `note`. If the note already has the same image
    /// (by hash), returns that one instead of storing a copy.
    pub(super) async fn add_image(
        &mut self,
        note: NoteId,
        image: PreparedImage,
    ) -> Result<NoteImage> {
        let mut rows = self
            .conn
            .query(
                &format!("{SELECT_IMAGES} WHERE image.note_id = ?1 AND image.sha256 = ?2"),
                (note.to_string(), image.sha256.as_str()),
            )
            .await?;
        if let Some(row) = rows.next().await? {
            return image_from_row(&row);
        }
        drop(rows);

        let id = ImageId::new();
        let now = Timestamp::now().as_millisecond();
        // All or nothing: no image without its bytes, and no bytes left over
        // if the note was deleted meanwhile.
        let tx = self.conn.transaction().await?;
        // Touching the note also checks it still exists.
        let changed = tx
            .execute(
                "UPDATE note SET updated_at = ?2 WHERE id = ?1 AND deleted_at IS NULL",
                (note.to_string(), now),
            )
            .await?;
        found(changed, "note")?;
        tx.execute(
            "INSERT INTO image (id, note_id, mime, width, height, sha256, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            (
                id.to_string(),
                note.to_string(),
                image.original.format.mime(),
                image.width,
                image.height,
                image.sha256.as_str(),
                now,
            ),
        )
        .await?;
        let versions = [
            ("thumb", Some(&image.thumb)),
            ("display", image.display.as_ref()),
            ("original", Some(&image.original)),
        ];
        for (kind, version) in versions {
            let Some(version) = version else { continue };
            tx.execute(
                "INSERT INTO image_data (image_id, kind, mime, bytes) VALUES (?1, ?2, ?3, ?4)",
                (
                    id.to_string(),
                    kind,
                    version.format.mime(),
                    version.bytes.as_slice(),
                ),
            )
            .await?;
        }
        tx.commit().await?;

        Ok(NoteImage {
            id,
            note_id: note,
            format: image.original.format,
            width: image.width,
            height: image.height,
            thumb: image.thumb,
        })
    }

    /// The version of an image to show full size: the display copy, or the
    /// original when it needed none.
    ///
    /// Two lookups rather than one query that picks between both rows:
    /// choosing in SQL would read the original's megabytes even when the
    /// display copy wins.
    pub(super) async fn display_image(&self, id: ImageId) -> Result<EncodedImage> {
        for kind in ["display", "original"] {
            let mut rows = self
                .conn
                .query(
                    "SELECT mime, bytes FROM image_data WHERE image_id = ?1 AND kind = ?2",
                    (id.to_string(), kind),
                )
                .await?;
            if let Some(row) = rows.next().await? {
                return encoded_image(&row, 0);
            }
        }
        Err(Error::NotFound("image"))
    }

    /// Removes an image and its bytes for good, and touches its note so it
    /// counts as changed.
    pub(super) async fn delete_image(&mut self, id: ImageId) -> Result<()> {
        let tx = self.conn.transaction().await?;
        tx.execute(
            "UPDATE note SET updated_at = ?2 \
             WHERE id = (SELECT note_id FROM image WHERE id = ?1)",
            (id.to_string(), Timestamp::now().as_millisecond()),
        )
        .await?;
        let changed = tx
            .execute("DELETE FROM image WHERE id = ?1", [id.to_string()])
            .await?;
        found(changed, "image")?;
        tx.commit().await?;
        Ok(())
    }

    async fn relink<T: Display>(&mut self, id: NoteId, links: &Links, ids: &[T]) -> Result<Note> {
        let tx = self.conn.transaction().await?;
        let changed = tx
            .execute(
                "UPDATE note SET updated_at = ?2 WHERE id = ?1 AND deleted_at IS NULL",
                (id.to_string(), Timestamp::now().as_millisecond()),
            )
            .await?;
        found(changed, "note")?;
        set_links(&tx, links, id, ids).await?;
        if links.table == CHILD_LINKS.table {
            // Shared with a child means about that child too.
            tx.execute(
                "DELETE FROM note_share WHERE note_id = ?1 AND child_id NOT IN \
                 (SELECT child_id FROM note_child WHERE note_id = ?1)",
                [id.to_string()],
            )
            .await?;
        }
        tx.commit().await?;
        self.note(id).await
    }

    /// The ids linked to one note, in the order `links` lists them.
    async fn linked_to<T: FromStr>(&self, links: &Links, note: NoteId) -> Result<Vec<T>> {
        let mut linked = self.linked(links, Some(note)).await?;
        Ok(linked.remove(&note).unwrap_or_default())
    }

    /// The ids linked to each note (or to just `note`), grouped by note.
    async fn linked<T: FromStr>(
        &self,
        links: &Links,
        note: Option<NoteId>,
    ) -> Result<HashMap<NoteId, Vec<T>>> {
        let only = if note.is_some() {
            "WHERE link.note_id = ?1"
        } else {
            ""
        };
        let sql = format!(
            "SELECT link.note_id, link.{column} FROM {table} link \
             JOIN {other} ON {other}.id = link.{column} {only} ORDER BY {order}",
            column = links.column,
            table = links.table,
            other = links.other,
            order = links.order,
        );
        let mut rows = self
            .conn
            .query(&sql, turso::params_from_iter(note.map(|id| id.to_string())))
            .await?;
        let mut linked: HashMap<NoteId, Vec<T>> = HashMap::new();
        while let Some(row) = rows.next().await? {
            linked
                .entry(parse_id(&row, 0, "note id")?)
                .or_default()
                .push(parse_id(&row, 1, links.what)?);
        }
        Ok(linked)
    }

    /// Fails with [`Error::DuplicateLabel`] if another label (not `except`)
    /// already uses `name`, ignoring case.
    ///
    /// The UNIQUE constraint would catch this too, but only as a generic
    /// database error. Checking first gives a clear message. There's no race:
    /// the database thread runs one request at a time.
    async fn ensure_label_name_free(&self, name: &str, except: Option<LabelId>) -> Result<()> {
        let mut rows = self
            .conn
            .query("SELECT id, name FROM label WHERE name = ?1", [name])
            .await?;
        while let Some(row) = rows.next().await? {
            let id: LabelId = parse_id(&row, 0, "label id")?;
            if Some(id) != except {
                return Err(Error::DuplicateLabel(row.get(1)?));
            }
        }
        Ok(())
    }
}

/// A table linking notes to children or labels.
struct Links {
    table: &'static str,
    /// The column holding the other side's id.
    column: &'static str,
    /// The table that column points to.
    other: &'static str,
    /// How to sort a note's linked rows: as the sidebar does.
    order: &'static str,
    /// For [`Error::Corrupt`].
    what: &'static str,
}

const CHILD_LINKS: Links = Links {
    table: "note_child",
    column: "child_id",
    other: "child",
    order: "child.sort_order, child.id",
    what: "child id",
};

/// Which children can see a note. Read only through `linked`; written by
/// `set_note_shared`.
const SHARE_LINKS: Links = Links {
    table: "note_share",
    column: "child_id",
    other: "child",
    order: "child.sort_order, child.id",
    what: "child id",
};

const LABEL_LINKS: Links = Links {
    table: "note_label",
    column: "label_id",
    other: "label",
    order: "label.name, label.id",
    what: "label id",
};

/// Replaces `note`'s links in `links.table` with `ids`. Call it inside a
/// transaction, so a failure leaves the old links in place.
///
/// The table names come from the constants above, never from input, so
/// `format!` can't inject SQL here.
async fn set_links<T: Display>(
    conn: &Connection,
    links: &Links,
    note: NoteId,
    ids: &[T],
) -> Result<()> {
    conn.execute(
        &format!("DELETE FROM {} WHERE note_id = ?1", links.table),
        [note.to_string()],
    )
    .await?;
    let insert = format!(
        "INSERT OR IGNORE INTO {} (note_id, {}) VALUES (?1, ?2)",
        links.table, links.column
    );
    for id in ids {
        conn.execute(&insert, (note.to_string(), id.to_string()))
            .await?;
    }
    Ok(())
}

fn child_from_row(row: &Row) -> Result<Child> {
    let color: i64 = row.get(2)?;
    Ok(Child {
        id: parse_id(row, 0, "child id")?,
        name: row.get(1)?,
        color: u32::try_from(color).map_err(|_| Error::Corrupt {
            what: "colour",
            value: color.to_string(),
        })?,
    })
}

/// Images with their thumbnails, as [`image_from_row`] reads them. Add a
/// `WHERE` clause.
pub(super) const SELECT_IMAGES: &str = "SELECT image.id, image.note_id, image.mime, image.width, \
     image.height, thumb.mime, thumb.bytes FROM image \
     JOIN image_data thumb ON thumb.image_id = image.id AND thumb.kind = 'thumb'";

fn image_from_row(row: &Row) -> Result<NoteImage> {
    Ok(NoteImage {
        id: parse_id(row, 0, "image id")?,
        note_id: parse_id(row, 1, "note id")?,
        format: image_format(row, 2)?,
        width: row.get(3)?,
        height: row.get(4)?,
        thumb: encoded_image(row, 5)?,
    })
}

/// Reads column `idx` as a MIME type.
fn image_format(row: &Row, idx: usize) -> Result<ImageFormat> {
    let mime: String = row.get(idx)?;
    ImageFormat::from_mime(&mime).ok_or(Error::Corrupt {
        what: "image type",
        value: mime,
    })
}

/// Reads an image's MIME type from column `idx` and its bytes from the next.
pub(super) fn encoded_image(row: &Row, idx: usize) -> Result<EncodedImage> {
    let format = image_format(row, idx)?;
    // turso has no `FromValue` for `Vec<u8>`; take the `Value` apart instead.
    match row.get_value(idx + 1)? {
        Value::Blob(bytes) => Ok(EncodedImage { format, bytes }),
        _ => Err(Error::Corrupt {
            what: "image",
            value: "something other than bytes".into(),
        }),
    }
}

/// Reads column `idx` as text and parses it into an id type.
pub(super) fn parse_id<T: FromStr>(row: &Row, idx: usize, what: &'static str) -> Result<T> {
    let text: String = row.get(idx)?;
    text.parse()
        .map_err(|_| Error::Corrupt { what, value: text })
}

/// Reads column `idx` as Unix milliseconds.
pub(super) fn timestamp(row: &Row, idx: usize) -> Result<Timestamp> {
    let millis: i64 = row.get(idx)?;
    Timestamp::from_millisecond(millis).map_err(|_| Error::Corrupt {
        what: "timestamp",
        value: millis.to_string(),
    })
}

/// Turns "the UPDATE/DELETE matched no rows" into [`Error::NotFound`].
pub(super) fn found(changed_rows: u64, what: &'static str) -> Result<()> {
    if changed_rows == 0 {
        return Err(Error::NotFound(what));
    }
    Ok(())
}

/// Opens the database at `location` with Turso, encrypted with `key` if
/// one is given. Doesn't touch the schema.
pub(super) async fn build(location: &Location, key: Option<&VaultKey>) -> Result<Database> {
    let path = match location {
        Location::File(path) => path
            .to_str()
            .ok_or_else(|| Error::NonUtf8Path(path.clone()))?,
        Location::Memory => ":memory:",
    };
    let mut builder = Builder::new_local(path);
    if let Some(key) = key {
        // Turso copies the key into a `String` of its own; ours is zeroed
        // when `hex` drops at the end of this block.
        let hex = key.hex();
        builder = builder
            .experimental_encryption(true)
            .with_encryption(EncryptionOpts {
                cipher: VAULT_CIPHER.to_owned(),
                hexkey: hex.to_string(),
            });
    }
    Ok(builder.build().await?)
}

/// Moves every commit from the write-ahead log into the database file and
/// empties the log, so the file alone holds the whole database.
pub(super) async fn checkpoint(conn: &Connection) -> Result<()> {
    let mut rows = conn.query("PRAGMA wal_checkpoint(TRUNCATE)", ()).await?;
    // One row: (busy, frames in the log, frames checkpointed).
    if let Some(row) = rows.next().await? {
        let busy: i64 = row.get(0)?;
        if busy != 0 {
            return Err(Error::Busy);
        }
    }
    Ok(())
}
