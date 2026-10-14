//! Small file operations the vault, its key file and the settings share.

use std::ffi::OsString;
use std::fs::{DirBuilder, File, OpenOptions};
use std::io::{ErrorKind, Write as _};
use std::path::{Path, PathBuf};

use crate::{Error, Result};

/// Writes `bytes` to `path` through a temporary file next to it, then
/// renames it into place. A rename within a folder is atomic, so a crash
/// halfway through leaves either the old file or the new one, never half
/// of each.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let temp = with_suffix(path, ".tmp");
    let write_error = |reason| Error::WriteFile {
        path: path.to_owned(),
        reason,
    };
    create_private(&temp)
        .and_then(|mut file| file.write_all(bytes))
        .map_err(write_error)?;
    std::fs::rename(&temp, path).map_err(|reason| {
        let _ = std::fs::remove_file(&temp);
        write_error(reason)
    })
}

/// Creates (or truncates) a file that, on Unix, only the current user can
/// read: the key file, backups, copies of the vault.
pub(crate) fn create_private(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(path)
}

/// `path` with `suffix` added to its file name: `vault.db` → `vault.db.tmp`.
pub(crate) fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name: OsString = path.file_name().unwrap_or_default().to_owned();
    name.push(suffix);
    path.with_file_name(name)
}

/// The write-ahead log Turso keeps next to a database: `vault.db-wal`.
pub(crate) fn wal_path(db: &Path) -> PathBuf {
    with_suffix(db, "-wal")
}

/// Deletes `path`; one that's already gone is fine.
pub(crate) fn remove_if_exists(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Err(err) if err.kind() != ErrorKind::NotFound => Err(Error::WriteFile {
            path: path.to_owned(),
            reason: err,
        }),
        _ => Ok(()),
    }
}

/// Renames `from` to `to`, replacing `to`.
pub(crate) fn rename(from: &Path, to: &Path) -> Result<()> {
    std::fs::rename(from, to).map_err(|reason| Error::WriteFile {
        path: to.to_owned(),
        reason,
    })
}

/// Creates `dir` and any missing parents. On Unix, new folders are readable
/// only by the current user: the vault holds private family notes.
pub(crate) fn create_private_dir(dir: &Path) -> Result<()> {
    let mut builder = DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(dir).map_err(|source| Error::CreateDir {
        path: dir.to_owned(),
        source,
    })
}
