//! Upgrades the vault's schema, one numbered step at a time.
//!
//! The schema version lives in SQLite's `user_version` header field. Each
//! migration runs in a transaction together with the bump of that number, so
//! a crash leaves the vault fully on the old version or fully on the new one.

use turso::Connection;

use crate::{Error, Result};

/// Migration `n` (counting from 1) upgrades a vault from version `n - 1` to
/// `n`. Append only: a migration that has shipped is never edited; changes go
/// in a new one.
const MIGRATIONS: &[&str] = &[
    include_str!("migrations/0001_children_and_labels.sql"),
    include_str!("migrations/0002_notes.sql"),
    include_str!("migrations/0003_images.sql"),
    include_str!("migrations/0004_sharing.sql"),
];

/// The schema version this build creates and understands.
pub(super) const LATEST: i64 = MIGRATIONS.len() as i64;

/// Brings the vault up to [`LATEST`], applying only the missing migrations.
pub(super) async fn run(conn: &mut Connection) -> Result<()> {
    let current = schema_version(conn).await?;
    if current > LATEST {
        // Written by a newer Papa: touching it could lose data.
        return Err(Error::SchemaTooNew {
            found: current,
            supported: LATEST,
        });
    }

    for (version, sql) in (1..=LATEST)
        .zip(MIGRATIONS)
        .skip_while(|(v, _)| *v <= current)
    {
        let tx = conn.transaction().await?;
        tx.execute_batch(sql).await?;
        // PRAGMA values can't be bound parameters, hence `format!`. `version`
        // is an integer we control, so this can't inject SQL.
        tx.execute_batch(&format!("PRAGMA user_version = {version}"))
            .await?;
        // Dropping `tx` without committing (on an error above) rolls back.
        tx.commit().await?;
    }
    Ok(())
}

pub(super) async fn schema_version(conn: &Connection) -> Result<i64> {
    let mut rows = conn.query("PRAGMA user_version", ()).await?;
    let row = rows
        .next()
        .await?
        .ok_or(turso::Error::QueryReturnedNoRows)?;
    Ok(row.get(0)?)
}
