//! The only module that writes SQL. It runs on the database thread; see
//! [`Store`](super::Store) for how requests get here.

use std::collections::HashMap;
use std::fmt::Display;
use std::fs::DirBuilder;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use jiff::Timestamp;
use turso::{Builder, Connection, Database, Row};

use super::migrations;
use crate::model::{SearchTerms, check_color, clean_child_name, clean_label_name, preview};
use crate::{
    Child, ChildDraft, ChildId, Error, Label, LabelId, Note, NoteDraft, NoteFilter, NoteId,
    NoteQuery, NoteSummary, Result,
};

/// Where the vault lives.
#[derive(Debug)]
pub(super) enum Location {
    File(PathBuf),
    /// Gone when the store closes. For tests and, later, demo mode.
    Memory,
}

pub(super) struct Db {
    conn: Connection,
    /// Kept alongside the connection so the database outlives it.
    _database: Database,
}

impl Db {
    /// Opens (or creates) the vault and brings its schema up to date.
    pub(super) async fn open(location: &Location) -> Result<Self> {
        let path = match location {
            Location::File(path) => {
                if let Some(dir) = path.parent() {
                    create_private_dir(dir)?;
                }
                path.to_str()
                    .ok_or_else(|| Error::NonUtf8Path(path.clone()))?
            }
            Location::Memory => ":memory:",
        };
        let database = Builder::new_local(path).build().await?;
        let mut conn = database.connect()?;
        // Off by default, as in SQLite, and set per connection. Without it
        // deleting a child would leave dangling `note_child` rows.
        conn.execute("PRAGMA foreign_keys = ON", ()).await?;
        migrations::run(&mut conn).await?;

        Ok(Self {
            conn,
            _database: database,
        })
    }

    #[cfg(test)]
    pub(super) fn connection(&self) -> &Connection {
        &self.conn
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
                created_at: timestamp(&row, 3)?,
            });
        }
        // A connection runs one statement at a time: `rows` must be gone
        // before the next query, or turso reports the connection busy.
        drop(rows);

        let mut children = self.linked(&CHILD_LINKS, None).await?;
        let mut labels = self.linked(&LABEL_LINKS, None).await?;
        for note in &mut notes {
            note.children = children.remove(&note.id).unwrap_or_default();
            note.labels = labels.remove(&note.id).unwrap_or_default();
        }
        Ok(notes)
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

    /// Replaces the children a note is about.
    pub(super) async fn set_note_children(
        &mut self,
        id: NoteId,
        children: &[ChildId],
    ) -> Result<Note> {
        self.relink(id, &CHILD_LINKS, children).await
    }

    /// Replaces a note's labels.
    pub(super) async fn set_note_labels(&mut self, id: NoteId, labels: &[LabelId]) -> Result<Note> {
        self.relink(id, &LABEL_LINKS, labels).await
    }

    /// Wipes a note's text and links and marks it deleted. See the `note`
    /// table in the migration for why the row stays.
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

/// Creates `dir` and any missing parents. On Unix, new folders are readable
/// only by the current user: the vault holds private family notes.
fn create_private_dir(dir: &Path) -> Result<()> {
    let mut builder = DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(dir).map_err(|source| Error::CreateDir {
        path: dir.to_owned(),
        source,
    })
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

/// Reads column `idx` as text and parses it into an id type.
fn parse_id<T: FromStr>(row: &Row, idx: usize, what: &'static str) -> Result<T> {
    let text: String = row.get(idx)?;
    text.parse()
        .map_err(|_| Error::Corrupt { what, value: text })
}

/// Reads column `idx` as Unix milliseconds.
fn timestamp(row: &Row, idx: usize) -> Result<Timestamp> {
    let millis: i64 = row.get(idx)?;
    Timestamp::from_millisecond(millis).map_err(|_| Error::Corrupt {
        what: "timestamp",
        value: millis.to_string(),
    })
}

/// Turns "the UPDATE/DELETE matched no rows" into [`Error::NotFound`].
fn found(changed_rows: u64, what: &'static str) -> Result<()> {
    if changed_rows == 0 {
        return Err(Error::NotFound(what));
    }
    Ok(())
}
