//! Setting, changing and removing the vault's password.
//!
//! Changing the password only rewraps the vault key (see [`crate::lock`]).
//! Setting the first password or removing it changes how every page is
//! stored, so the vault is rewritten: copied row by row into a new file,
//! encrypted or plain, which then replaces the old one.
//!
//! ```text
//!  1. copy every row into vault.db.new        nothing has changed yet
//!  2. write vault.key (when encrypting)       a stray key file is harmless
//!  3. close vault.db, rename .new over it     the one atomic step
//!  4. delete vault.key (when decrypting)
//!  5. reopen
//! ```
//!
//! A crash at any point leaves a vault that opens: the old one before
//! step 3, the new one after. Which one it is, and so which key it needs,
//! is read from the file's first bytes.

use std::path::Path;

use turso::Connection;

use super::db::{self, Db, Location};
use super::migrations;
use crate::files::{self, remove_if_exists, wal_path, with_suffix};
use crate::lock::{self, KeyFile, Password, VaultKey, VaultKind, key_path};
use crate::{Error, Result};

/// Gives a plain vault its first password, or changes the password of an
/// encrypted one. `current` must be the current password if there is one.
///
/// Returns the vault to use from now on: `None` only if it couldn't be
/// reopened after the swap, which leaves the store closed.
pub(super) async fn set_password(
    db: Db,
    current: Option<Password>,
    new: Password,
) -> (Option<Db>, Result<()>) {
    if let Err(err) = new.check_new() {
        return (Some(db), Err(err));
    }
    let Some(path) = db.location().path().map(Path::to_owned) else {
        return (Some(db), Err(Error::InMemoryVault));
    };
    let key_file = key_path(&path);

    if let Some(key) = db.key() {
        // Already encrypted: check the current password, then wrap the same
        // key under the new one. The vault itself doesn't change.
        let result = (|| -> Result<()> {
            let current = current.ok_or(Error::WrongPassword)?;
            KeyFile::read(&key_file)?.unwrap(&current)?;
            KeyFile::wrap(key, &new)?.write(&key_file)
        })();
        return (Some(db), result);
    }

    // A new key for a newly encrypted vault, wrapped before anything else
    // happens: if this fails, nothing has.
    let wrapped = VaultKey::generate().and_then(|key| Ok((KeyFile::wrap(&key, &new)?, key)));
    let (wrapped, key) = match wrapped {
        Ok(pair) => pair,
        Err(err) => return (Some(db), Err(err)),
    };
    rewrite(db, &path, Some(key), Some(wrapped)).await
}

/// Decrypts the vault, after checking `current` is its password. A vault
/// without a password is left as it is.
pub(super) async fn remove_password(db: Db, current: Password) -> (Option<Db>, Result<()>) {
    let Some(path) = db.location().path().map(Path::to_owned) else {
        return (Some(db), Err(Error::InMemoryVault));
    };
    if db.key().is_none() {
        return (Some(db), Ok(()));
    }
    if let Err(err) = KeyFile::read(&key_path(&path)).and_then(|file| file.unwrap(&current)) {
        return (Some(db), Err(err));
    }
    rewrite(db, &path, None, None).await
}

/// Replaces the vault at `path` with a copy encrypted with `key` (or plain,
/// for `None`), and reopens it. `key_file` is written before the swap.
async fn rewrite(
    db: Db,
    path: &Path,
    key: Option<VaultKey>,
    key_file: Option<KeyFile>,
) -> (Option<Db>, Result<()>) {
    let temp = with_suffix(path, ".new");
    let built = async {
        copy_into(db.connection(), &temp, key.as_ref()).await?;
        if let Some(file) = &key_file {
            file.write(&key_path(path))?;
        }
        Ok(())
    }
    .await;
    if let Err(err) = built {
        discard(&temp);
        return (Some(db), Err(err));
    }

    let old_key = db.key().cloned();
    let swapped = async {
        db.close().await?;
        // Empty after the checkpoint, but it belongs to the old file: left
        // next to the new one, Turso would replay it into it.
        remove_if_exists(&wal_path(path))?;
        files::rename(&temp, path)?;
        if key_file.is_none() {
            remove_if_exists(&key_path(path))?;
        }
        Ok(())
    }
    .await;
    if swapped.is_err() {
        discard(&temp);
    }

    // Whichever vault is in place now, open it with the key that fits.
    let key = match lock::inspect(path) {
        Ok(VaultKind::Encrypted) => key.or(old_key),
        _ => None,
    };
    match Db::open(&Location::File(path.to_owned()), key).await {
        Ok(db) => (Some(db), swapped),
        Err(err) => (None, swapped.and(Err(err))),
    }
}

/// Writes a copy of the database behind `source` to a new file at `dest`,
/// encrypted with `key` if one is given.
///
/// The copy gets its schema from the migrations, which is the source's
/// (it was migrated when it opened), then every row of every table. One
/// transaction, so it's quick; foreign keys are off on this connection,
/// so the order of the tables doesn't matter.
pub(super) async fn copy_into(
    source: &Connection,
    dest: &Path,
    key: Option<&VaultKey>,
) -> Result<()> {
    discard(dest);
    let database = db::build(&Location::File(dest.to_owned()), key).await?;
    let mut conn = database.connect()?;
    migrations::run(&mut conn).await?;

    let tx = conn.transaction().await?;
    for table in tables(source).await? {
        copy_table(source, &tx, &table).await?;
    }
    tx.commit().await?;
    db::checkpoint(&conn).await?;
    drop(conn);
    drop(database);
    remove_if_exists(&wal_path(dest))
}

/// The tables of the database, in the order they were created.
async fn tables(conn: &Connection) -> Result<Vec<String>> {
    let mut rows = conn
        .query(
            "SELECT name FROM sqlite_schema \
             WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY rowid",
            (),
        )
        .await?;
    let mut tables = Vec::new();
    while let Some(row) = rows.next().await? {
        tables.push(row.get(0)?);
    }
    Ok(tables)
}

/// Copies every row of `table`, one at a time: an image's bytes are only
/// in memory while that row is copied.
async fn copy_table(source: &Connection, dest: &Connection, table: &str) -> Result<()> {
    // The names come from the schema, which only our migrations write;
    // quoting them is still the right thing to do.
    let quoted = format!("\"{}\"", table.replace('"', "\"\""));
    let mut rows = source.query(&format!("SELECT * FROM {quoted}"), ()).await?;
    let mut insert = None;
    while let Some(row) = rows.next().await? {
        let columns = row.column_count();
        let insert = insert.get_or_insert_with(|| {
            let params: Vec<String> = (1..=columns).map(|n| format!("?{n}")).collect();
            format!("INSERT INTO {quoted} VALUES ({})", params.join(", "))
        });
        let values = (0..columns)
            .map(|idx| row.get_value(idx))
            .collect::<turso::Result<Vec<_>>>()?;
        dest.execute(insert, turso::params_from_iter(values))
            .await?;
    }
    Ok(())
}

/// Removes a half-made copy and its log.
fn discard(path: &Path) {
    let _ = remove_if_exists(path);
    let _ = remove_if_exists(&wal_path(path));
}
