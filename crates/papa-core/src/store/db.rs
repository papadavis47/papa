//! The only module that writes SQL. It runs on the database thread; see
//! [`Store`](super::Store) for how requests get here.

use std::fs::DirBuilder;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use turso::{Builder, Connection, Database, Row};

use super::migrations;
use crate::model::{check_color, clean_child_name, clean_label_name};
use crate::{Child, ChildDraft, ChildId, Error, Label, LabelId, Result};

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
        // Off by default, as in SQLite, and set per connection. No table
        // references another yet, but Phase 3's notes will.
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

/// Turns "the UPDATE/DELETE matched no rows" into [`Error::NotFound`].
fn found(changed_rows: u64, what: &'static str) -> Result<()> {
    if changed_rows == 0 {
        return Err(Error::NotFound(what));
    }
    Ok(())
}
