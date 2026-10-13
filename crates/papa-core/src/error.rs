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

    #[error("the image is larger than 50 MB")]
    ImageTooLarge,

    #[error("the image has more than 100 megapixels")]
    ImageTooManyPixels,

    #[error(
        "Papa can't read HEIC photos yet. Export it as JPEG first, or set your \
         iPhone to save JPEG: Settings → Camera → Formats → Most Compatible"
    )]
    HeicImage,

    #[error("that isn't an image Papa can read; use JPEG, PNG, GIF or WebP")]
    UnsupportedImage,

    /// The file looks like a supported image but doesn't decode.
    #[error("the image is damaged or incomplete ({0})")]
    InvalidImage(String),

    /// The reason is part of the message, not a `#[source]`: the app shows
    /// it in a notification, which displays only the top error.
    #[error("couldn't read {}: {reason}", path.display())]
    ReadFile {
        path: PathBuf,
        reason: std::io::Error,
    },

    #[error("couldn't write {}: {reason}", path.display())]
    WriteFile {
        path: PathBuf,
        reason: std::io::Error,
    },

    /// `serde_json`'s message says which line and column.
    #[error("{} isn't valid: {reason}", path.display())]
    InvalidSettings {
        path: PathBuf,
        reason: serde_json::Error,
    },

    #[error("couldn't find the system data folder; set PAPA_VAULT to a vault file path")]
    NoDataDir,

    #[error("couldn't find the system config folder; set PAPA_CONFIG_DIR to a folder path")]
    NoConfigDir,

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
