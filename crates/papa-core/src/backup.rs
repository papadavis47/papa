//! Backups of the vault: one file to keep somewhere else, and putting one
//! back.
//!
//! A backup is a short header followed by the vault file, byte for byte:
//!
//! ```text
//!  PAPA BACKUP\n
//!  {"version":1,"created_at":…,"schema_version":3,"key":null}\n
//!  <the vault: a SQLite file, or a Turso-encrypted one>
//! ```
//!
//! - **A plain vault** gives a plain backup: after the header, an ordinary
//!   SQLite database that any SQLite tool opens. Turso's FAQ advises keeping
//!   backups that don't depend on Turso while it's pre-1.0.
//! - **An encrypted vault** gives an encrypted backup: the vault's pages as
//!   they are on disk, plus its key file in the header. It opens with the
//!   password the vault had when the backup was made. Nothing is ever
//!   decrypted to make it, so no plaintext touches the disk.
//!
//! The vault is copied after a checkpoint has moved every commit into the
//! file, by the database thread, which runs nothing else meanwhile: the
//! copy is a consistent snapshot.

use std::fs::File;
use std::io::{BufRead as _, BufReader, ErrorKind, Read as _, Write as _};
use std::path::{Path, PathBuf};

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::files::{self, remove_if_exists, wal_path, with_suffix};
use crate::lock::{KeyFile, Password, key_path};
use crate::{Error, Result, Store};

/// The first line of every backup.
const MAGIC: &[u8] = b"PAPA BACKUP\n";

/// Headers are a few hundred bytes; anything much longer isn't one.
const MAX_HEADER: u64 = 64 * 1024;

/// The extension Papa gives backups.
pub const EXTENSION: &str = "papabackup";

#[derive(Debug, Serialize, Deserialize)]
struct Header {
    version: u32,
    /// Unix milliseconds.
    created_at: i64,
    /// The vault's schema version, so a backup from a newer Papa is refused
    /// before anything is replaced.
    schema_version: i64,
    /// The vault's key file, if it's encrypted.
    key: Option<KeyFile>,
}

/// What a backup's header says about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackupInfo {
    pub created_at: Timestamp,
    /// Restoring it needs the password the vault had when it was made.
    pub encrypted: bool,
}

/// A suggested file name: `Papa backup 2026-10-14.papabackup`.
pub fn file_name(date: jiff::civil::Date) -> String {
    format!("Papa backup {date}.{EXTENSION}")
}

/// Writes a backup of the vault file at `vault` to `dest`. Call it on the
/// database thread, right after a checkpoint, so nothing changes the vault
/// while it's copied.
pub(crate) fn write(
    vault: &Path,
    key: Option<KeyFile>,
    schema_version: i64,
    dest: &Path,
) -> Result<()> {
    let header = Header {
        version: 1,
        created_at: Timestamp::now().as_millisecond(),
        schema_version,
        key,
    };
    let header = serde_json::to_string(&header).expect("a header is plain data and serializes");
    let mut source = File::open(vault).map_err(|reason| Error::ReadFile {
        path: vault.to_owned(),
        reason,
    })?;

    // Written under a temporary name, then renamed: a backup that's there
    // is a whole one.
    let temp = with_suffix(dest, ".part");
    let written = (|| {
        let mut out = std::io::BufWriter::new(files::create_private(&temp)?);
        out.write_all(MAGIC)?;
        out.write_all(header.as_bytes())?;
        out.write_all(b"\n")?;
        std::io::copy(&mut source, &mut out)?;
        // On disk, not just handed to the OS: the father may unplug the
        // drive as soon as Papa says it's done.
        out.into_inner()?.sync_all()
    })();
    if let Err(reason) = written {
        let _ = std::fs::remove_file(&temp);
        return Err(Error::WriteFile {
            path: dest.to_owned(),
            reason,
        });
    }
    files::rename(&temp, dest)
}

/// Reads a backup's header.
pub fn inspect(path: &Path) -> Result<BackupInfo> {
    let (header, _) = open(path)?;
    Ok(BackupInfo {
        created_at: Timestamp::from_millisecond(header.created_at)
            .map_err(|_| Error::NotABackup(path.to_owned()))?,
        encrypted: header.key.is_some(),
    })
}

/// Opens a backup and reads its header, leaving the reader at the start of
/// the vault's bytes.
fn open(path: &Path) -> Result<(Header, BufReader<File>)> {
    let read_error = |reason| Error::ReadFile {
        path: path.to_owned(),
        reason,
    };
    let not_a_backup = || Error::NotABackup(path.to_owned());
    let mut reader = BufReader::new(File::open(path).map_err(read_error)?);

    let mut magic = [0u8; MAGIC.len()];
    match reader.read_exact(&mut magic) {
        Err(err) if err.kind() == ErrorKind::UnexpectedEof => return Err(not_a_backup()),
        result => result.map_err(read_error)?,
    }
    if magic != MAGIC {
        return Err(not_a_backup());
    }
    let mut line = Vec::new();
    (&mut reader)
        .take(MAX_HEADER)
        .read_until(b'\n', &mut line)
        .map_err(read_error)?;
    let header: Header = serde_json::from_slice(&line).map_err(|_| not_a_backup())?;
    if header.version != 1 {
        return Err(Error::BackupTooNew);
    }
    Ok((header, reader))
}

/// A backup copied next to the vault and checked, ready to replace it.
/// Dropping it without [`commit`](Self::commit) deletes the copy.
#[derive(Debug)]
pub struct PreparedRestore {
    vault: PathBuf,
    /// The checked copy: `vault.db.restore`.
    temp: PathBuf,
    key: Option<KeyFile>,
    committed: bool,
}

/// Copies the backup at `backup` next to the vault at `vault` and checks it
/// opens: with `password` if it's encrypted, which must be the password the
/// vault had when the backup was made. A backup from an older Papa is
/// brought up to date on the way.
///
/// Nothing about the current vault changes, so it can stay open meanwhile.
pub async fn prepare_restore(
    backup: &Path,
    vault: &Path,
    password: Option<&Password>,
) -> Result<PreparedRestore> {
    let (header, mut reader) = open(backup)?;
    if header.schema_version > crate::store::LATEST_SCHEMA {
        return Err(Error::SchemaTooNew {
            found: header.schema_version,
            supported: crate::store::LATEST_SCHEMA,
        });
    }
    // Before copying anything: the password is the likeliest mistake.
    let key = match &header.key {
        Some(file) => Some(file.unwrap(password.ok_or(Error::WrongPassword)?)?),
        None => None,
    };

    if let Some(dir) = vault.parent() {
        files::create_private_dir(dir)?;
    }
    let prepared = PreparedRestore {
        vault: vault.to_owned(),
        temp: with_suffix(vault, ".restore"),
        key: header.key,
        committed: false,
    };
    prepared.discard();
    let copied = files::create_private(&prepared.temp).and_then(|mut out| {
        std::io::copy(&mut reader, &mut out)?;
        out.sync_all()
    });
    copied.map_err(|reason| Error::WriteFile {
        path: prepared.temp.clone(),
        reason,
    })?;

    // Opening it runs the migrations; reading every table checks that its
    // pages decrypt and its rows make sense.
    let store = Store::open_with_key(&prepared.temp, key).await?;
    let checked = store.check().await;
    store.close().await?;
    checked?;
    Ok(prepared)
}

impl PreparedRestore {
    /// Where the vault that's replaced is kept: `vault-before-restore.db`,
    /// with its key file and log, if any.
    pub fn kept_path(&self) -> PathBuf {
        let stem = self.vault.file_stem().unwrap_or_default().to_string_lossy();
        let name = match self.vault.extension() {
            Some(ext) => format!("{stem}-before-restore.{}", ext.to_string_lossy()),
            None => format!("{stem}-before-restore"),
        };
        self.vault.with_file_name(name)
    }

    /// Puts the backup in place of the vault, which must be closed. The
    /// replaced vault is kept at [`kept_path`](Self::kept_path), replacing
    /// whatever an earlier restore kept there.
    pub fn commit(mut self) -> Result<()> {
        let kept = self.kept_path();
        for path in [&kept, &wal_path(&kept), &key_path(&kept)] {
            remove_if_exists(path)?;
        }
        let moves = [
            (self.vault.clone(), kept.clone()),
            (wal_path(&self.vault), wal_path(&kept)),
            (key_path(&self.vault), key_path(&kept)),
        ];
        let mut moved = Vec::new();
        for (from, to) in &moves {
            if from.exists() {
                if let Err(err) = files::rename(from, to) {
                    undo(&moved);
                    return Err(err);
                }
                moved.push((from, to));
            }
        }

        // The copy's log, emptied when the check closed it.
        let _ = remove_if_exists(&wal_path(&self.temp));
        let placed = files::rename(&self.temp, &self.vault).and_then(|()| match &self.key {
            Some(file) => file.write(&key_path(&self.vault)),
            None => Ok(()),
        });
        if let Err(err) = placed {
            let _ = remove_if_exists(&self.vault);
            undo(&moved);
            return Err(err);
        }
        self.committed = true;
        Ok(())
    }

    fn discard(&self) {
        let _ = remove_if_exists(&self.temp);
        let _ = remove_if_exists(&wal_path(&self.temp));
    }
}

/// Moves files back where they were.
fn undo(moved: &[(&PathBuf, &PathBuf)]) {
    for (from, to) in moved {
        let _ = std::fs::rename(to, from);
    }
}

impl Drop for PreparedRestore {
    fn drop(&mut self) {
        if !self.committed {
            self.discard();
        }
    }
}
