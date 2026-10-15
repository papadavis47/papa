//! Domain logic for Papa, independent of any UI.
//!
//! - [`model`]: children, labels and notes, and the rules they follow.
//! - [`Store`]: the local vault on the `turso` crate. It runs on its own
//!   database thread; the UI talks to it through this cheap, cloneable handle.
//! - [`images`]: turns attached photos into what the vault stores.
//! - [`settings`]: the father's preferences and their file, `settings.json`.
//! - [`paths`]: where the vault and the settings live on disk.
//! - [`lock`]: the vault's password, and the key file it unlocks.
//! - [`backup`]: backups of the vault, and restoring one.
//! - [`share`]: access keys, the keys derived from them, sealing notes for
//!   a child's cloud database, and the share projection.
//! - [`Sharing`]: the father's side of sharing, on its own thread: his
//!   relay account, each child's share, and syncing it.
//! - [`relay`]: the relay's HTTP API.
//!
//! Planned (see `PLAN.md`): the child's side of sharing, and messages.

pub mod backup;
mod error;
mod files;
pub mod images;
pub mod lock;
pub mod model;
pub mod paths;
pub mod relay;
pub mod settings;
pub mod share;
mod sharing;
mod store;
mod tls;

pub use error::{Error, Result};
pub use images::PreparedImage;
pub use model::{
    Account, Child, ChildDraft, ChildId, ChildShare, EncodedImage, ImageFormat, ImageId, Label,
    LabelId, Note, NoteDraft, NoteFilter, NoteId, NoteImage, NoteQuery, NoteSummary,
};
pub use sharing::{Sharing, SyncReport};
pub use store::Store;
