//! Domain logic for Papa, independent of any UI.
//!
//! - [`model`]: children and labels, and the rules their names follow.
//! - [`Store`]: the local vault on the `turso` crate. It runs on its own
//!   database thread; the UI talks to it through this cheap, cloneable handle.
//! - [`paths`]: where the vault lives on disk.
//!
//! Planned (see `PLAN.md`): notes and attachments, crypto for shared notes,
//! the share projection, the relay client and the settings file.

mod error;
pub mod model;
pub mod paths;
mod store;

pub use error::{Error, Result};
pub use model::{Child, ChildDraft, ChildId, Label, LabelId};
pub use store::Store;
