//! Sharing notes with a child (PLAN.md §2.2, ADR 0004 and 0015): the
//! access key, the keys derived from it, sealing notes and messages for
//! the cloud, working out what a child's share database is missing, and
//! reading it back on the child's side.

mod access_key;
mod key_card;
mod keys;
pub mod message;
pub mod projection;
pub(crate) mod replica;

pub use access_key::AccessKey;
pub use key_card::KeyCard;
pub use keys::{Aad, AuthKey, ChildKeys, ContentKey, EncKey, Sealed};
pub use message::{MessageVersion, SealedMessage};
pub use projection::{BlobKind, NotePayload};
