use std::path::PathBuf;

use crate::model::MAX_NAME_CHARS;

/// Everything that can go wrong in `papa-core`.
///
/// Callers can `match` on the variant, e.g. to keep a dialog open on
/// [`Error::DuplicateLabel`]. Messages are written to be shown to the user.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("the name can't be empty")]
    EmptyName,

    #[error("the name can be at most {MAX_NAME_CHARS} characters long")]
    NameTooLong,

    #[error("{0:#08x} isn't a colour; expected 0xRRGGBB")]
    InvalidColor(u32),

    #[error("a label named “{0}” already exists")]
    DuplicateLabel(String),

    /// The row was deleted, e.g. by another window, before the request ran.
    #[error("that {0} no longer exists")]
    NotFound(&'static str),

    #[error(
        "this vault was created by a newer version of Papa \
         (schema version {found}; this version understands up to {supported})"
    )]
    SchemaTooNew { found: i64, supported: i64 },

    #[error("the vault contains an invalid {what}: {value:?}")]
    Corrupt { what: &'static str, value: String },

    #[error("couldn't find the system data folder; set PAPA_VAULT to a vault file path")]
    NoDataDir,

    #[error("the vault path must be valid UTF-8: {}", .0.display())]
    NonUtf8Path(PathBuf),

    #[error("couldn't create the folder {}", path.display())]
    CreateDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("couldn't start the database thread")]
    StartThread(#[source] std::io::Error),

    /// The database thread has exited (it panicked, or failed to open), so
    /// requests have nowhere to go.
    #[error("the database thread has stopped")]
    StoreClosed,

    /// An error from the `turso` engine, shown as-is.
    #[error(transparent)]
    Database(#[from] turso::Error),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
