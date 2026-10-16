use std::path::PathBuf;

use crate::lock::MIN_PASSWORD_CHARS;
use crate::model::{MAX_MESSAGE_CHARS, MAX_NAME_CHARS};

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

    #[error("the message is empty")]
    EmptyMessage,

    #[error("a message can be at most {MAX_MESSAGE_CHARS} characters long")]
    MessageTooLong,

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

    #[error("the vault is locked; unlock it with its password")]
    Locked,

    #[error("that isn't the password")]
    WrongPassword,

    #[error("the password must be at least {MIN_PASSWORD_CHARS} characters long")]
    PasswordTooShort,

    /// An encrypted vault without its key file can't be opened, password or
    /// not: the key file holds the vault's key.
    #[error(
        "the vault is encrypted, but its key file {} is missing. \
         Put it back, or restore a backup",
        .0.display()
    )]
    KeyFileMissing(PathBuf),

    #[error("the vault's key file isn't valid: {0}")]
    InvalidKeyFile(String),

    #[error("couldn't get random bytes from the system: {0}")]
    Random(getrandom::Error),

    #[error("this vault is only in memory, so it can't be encrypted or backed up")]
    InMemoryVault,

    #[error("{} isn't a Papa backup", .0.display())]
    NotABackup(PathBuf),

    #[error("this backup was made by a newer version of Papa")]
    BackupTooNew,

    /// A checkpoint couldn't finish. Only Papa uses the vault, so this
    /// shouldn't happen.
    #[error("the vault is busy; try again")]
    Busy,

    #[error("that isn't a Papa access key; check it was copied in full")]
    InvalidAccessKey,

    /// Opening a sealed note, image or key failed: the wrong key, or the
    /// data was changed on the way.
    #[error("couldn't decrypt {0}: the key is wrong, or the data was changed")]
    Decrypt(&'static str),

    /// The relay refused a request. `message` is the relay's, for people;
    /// match on `code`.
    #[error("{}", relay_message(*.code, .message))]
    Relay {
        code: papa_proto::ErrorCode,
        message: String,
    },

    /// The relay or a share's database couldn't be reached.
    #[error("couldn't reach the server: {0}")]
    Network(String),

    #[error("sign in to your Papa account first, in Settings → Sharing")]
    NotSignedIn,

    /// A child's app was asked to read before anyone signed in to a share.
    #[error("not signed in to the notes shared with you")]
    ChildSignedOut,

    #[error("sharing with this child was set up on {0}; sign in to that server to manage it")]
    WrongRelay(String),

    #[error("the server address must start with https:// (or http:// for one on this computer)")]
    InvalidRelayUrl,

    #[error("{0} is still shared. Stop sharing first, so their copy in the cloud is deleted too")]
    ChildIsShared(String),

    /// Sharing a note with a child it isn't about, or who isn't shared.
    #[error("the note can only be shared with a child it's about, once sharing with them is on")]
    CantShareNote,

    /// Messages go to a child whose sharing is on: the share is how they
    /// travel.
    #[error("turn sharing on for {0} to send them messages")]
    NotShared(String),

    /// On the child's side: Dad's Papa hasn't made the share's message
    /// tables yet (an older version), or his new key hasn't reached it.
    #[error("messages open once Dad's Papa has synced; try again later")]
    MessagesUnavailable,

    /// Turso's sync failed while pushing a share.
    #[error("couldn't sync the shared notes: {0}")]
    Sync(String),

    /// An error from the `turso` engine, shown as-is.
    #[error(transparent)]
    Database(#[from] turso::Error),
}

/// A relay error for people: the relay's own message, except where the
/// app knows better what to do about it.
fn relay_message(code: papa_proto::ErrorCode, message: &str) -> String {
    use papa_proto::ErrorCode::*;
    match code {
        InvalidCredentials => "that email doesn't match that password or access key".to_owned(),
        InvalidInvite => "that invite code is unknown, used or expired".to_owned(),
        EmailTaken => "there's already an account with that email; sign in instead".to_owned(),
        RateLimited => "too many attempts; wait a minute and try again".to_owned(),
        Unauthorized => "your session has ended; sign in again in Settings → Sharing".to_owned(),
        _ => message.to_owned(),
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
