//! Messages in a share database: sealing one, and opening it again.
//!
//! A message row is append-only. Whoever writes a message makes its row,
//! and the recipient makes its `receipt` row when they read it, so the
//! father's and the child's devices never write the same row and Turso's
//! last-push-wins never has to choose. The one exception is a new key:
//! Dad's app seals every message again under it, the child's included,
//! so the child can still read the whole conversation.
//!
//! A row's sender, time and key epoch are in the clear, next to the
//! sealed body, and bound to it: see [`Aad::Message`].

use std::fmt;

use serde::{Deserialize, Serialize};

use super::{Aad, ContentKey, Sealed};
use crate::{Error, MessageId, Result, Sender};

/// A message as sealed: JSON inside the ciphertext, so later versions can
/// add to it.
#[derive(Serialize, Deserialize)]
struct MessagePayload {
    body: String,
}

/// Which version of a message a row holds: who wrote it, and the key
/// epoch it's sealed under.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MessageVersion {
    pub sender: Sender,
    pub epoch: i64,
}

/// A message row.
#[derive(Clone)]
pub struct SealedMessage {
    pub id: MessageId,
    pub sender: Sender,
    /// Unix milliseconds, by the writer's clock.
    pub created_at: i64,
    pub epoch: i64,
    pub sealed: Sealed,
}

impl fmt::Debug for SealedMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SealedMessage({}, {:?})", self.id, self.sender)
    }
}

impl SealedMessage {
    /// Seals `body` for `share`, under `key` of `epoch`.
    pub fn seal(
        key: &ContentKey,
        share: &str,
        id: MessageId,
        sender: Sender,
        created_at: i64,
        epoch: i64,
        body: &str,
    ) -> Result<Self> {
        let payload = zeroize::Zeroizing::new(
            serde_json::to_vec(&MessagePayload {
                body: body.to_owned(),
            })
            .expect("a message payload serializes"),
        );
        let message = id.to_string();
        let aad = Aad::Message {
            share,
            message: &message,
            sender: sender.as_str(),
            created_at,
            epoch,
        };
        Ok(Self {
            id,
            sender,
            created_at,
            epoch,
            sealed: key.seal(aad, &payload)?,
        })
    }

    /// The body, if `key` is this row's and nothing was changed.
    pub fn open(&self, key: &ContentKey, share: &str) -> Result<String> {
        let message = self.id.to_string();
        let aad = Aad::Message {
            share,
            message: &message,
            sender: self.sender.as_str(),
            created_at: self.created_at,
            epoch: self.epoch,
        };
        let json = zeroize::Zeroizing::new(key.open(aad, &self.sealed, "a message")?);
        let payload: MessagePayload =
            serde_json::from_slice(&json).map_err(|err| Error::Corrupt {
                what: "message",
                value: err.to_string(),
            })?;
        Ok(payload.body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHARE: &str = "0199d3c4-0000-7000-8000-000000000001";

    #[test]
    fn a_message_opens_only_as_written() {
        let key = ContentKey::generate().unwrap();
        let id = MessageId::new();
        let sealed =
            SealedMessage::seal(&key, SHARE, id, Sender::Child, 1_000, 2, "Hi Dad").unwrap();
        assert_eq!(sealed.open(&key, SHARE).unwrap(), "Hi Dad");

        // The server can see who wrote it and when, but not change either.
        let as_father = SealedMessage {
            sender: Sender::Father,
            ..sealed.clone()
        };
        assert!(as_father.open(&key, SHARE).is_err());
        let moved = SealedMessage {
            created_at: 999,
            ..sealed.clone()
        };
        assert!(moved.open(&key, SHARE).is_err());
        let other_epoch = SealedMessage {
            epoch: 1,
            ..sealed.clone()
        };
        assert!(other_epoch.open(&key, SHARE).is_err());
        assert!(sealed.open(&key, "another share").is_err());
        assert!(
            sealed
                .open(&ContentKey::generate().unwrap(), SHARE)
                .is_err()
        );
    }
}
