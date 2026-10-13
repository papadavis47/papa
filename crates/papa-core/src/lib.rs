//! Domain logic for Papa, independent of any UI.
//!
//! - [`model`]: children, labels and notes, and the rules they follow.
//! - [`Store`]: the local vault on the `turso` crate. It runs on its own
//!   database thread; the UI talks to it through this cheap, cloneable handle.
//! - [`images`]: turns attached photos into what the vault stores.
//! - [`settings`]: the father's preferences and their file, `settings.json`.
//! - [`paths`]: where the vault and the settings live on disk.
//!
//! Planned (see `PLAN.md`): crypto for shared notes,
//! the share projection and the relay client.

mod error;
pub mod images;
pub mod model;
pub mod paths;
pub mod settings;
mod store;

pub use error::{Error, Result};
pub use images::PreparedImage;
pub use model::{
    Child, ChildDraft, ChildId, EncodedImage, ImageFormat, ImageId, Label, LabelId, Note,
    NoteDraft, NoteFilter, NoteId, NoteImage, NoteQuery, NoteSummary,
};
pub use store::Store;
