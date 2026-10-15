//! Sharing notes with a child (PLAN.md §2.2, ADR 0004 and 0015): the
//! access key, the keys derived from it, sealing notes for the cloud, and
//! working out what a child's share database is missing.

mod access_key;
mod keys;
pub mod projection;
pub(crate) mod replica;

pub use access_key::AccessKey;
pub use keys::{Aad, AuthKey, ChildKeys, ContentKey, EncKey, Sealed};
pub use projection::{BlobKind, NotePayload};
