//! The vault's side of messages: the conversation with each child, and
//! what travels through the share's database.
//!
//! A sync receives first ([`Db::receive`]): the child's new messages, and
//! receipts for the father's. Then the projection
//! ([`Db::message_changes`]) seals whatever the share doesn't have yet,
//! like it does notes. The vault is the father's record; the share only
//! carries messages between the two apps.

use std::collections::{HashMap, HashSet};

use jiff::Timestamp;
use turso::Row;

use super::db::{Db, found, parse_id, timestamp};
use crate::model::clean_message;
use crate::share::projection::{Manifest, ShareChanges};
use crate::share::{ContentKey, SealedMessage};
use crate::{ChildId, ConversationSummary, Error, Message, MessageId, Result, Sender};

/// What a sync received from a child.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Received {
    pub messages: usize,
    pub receipts: usize,
}

impl Db {
    /// The conversation with `child`, oldest first.
    pub(super) async fn messages(&self, child: ChildId) -> Result<Vec<Message>> {
        let mut rows = self
            .conn
            .query(
                "SELECT id, sender, body, created_at, sent_at, read_at FROM message \
                 WHERE child_id = ?1 ORDER BY created_at, id",
                [child.to_string()],
            )
            .await?;
        let mut messages = Vec::new();
        while let Some(row) = rows.next().await? {
            messages.push(message_from_row(&row)?);
        }
        Ok(messages)
    }

    /// Adds a message from the father, made by the UI as `id`. It goes out
    /// with the next sync.
    pub(super) async fn add_message(
        &self,
        child: ChildId,
        id: MessageId,
        body: &str,
    ) -> Result<Message> {
        let body = clean_message(body)?;
        if self.share(child).await?.is_none() {
            let name = self
                .children()
                .await?
                .into_iter()
                .find(|c| c.id == child)
                .ok_or(Error::NotFound("child"))?
                .name;
            return Err(Error::NotShared(name));
        }
        let message = Message::draft(id, Sender::Father, body);
        self.conn
            .execute(
                "INSERT INTO message (id, child_id, sender, body, created_at) \
                 VALUES (?1, ?2, 'father', ?3, ?4)",
                (
                    id.to_string(),
                    child.to_string(),
                    message.body.as_str(),
                    message.created_at.as_millisecond(),
                ),
            )
            .await?;
        Ok(message)
    }

    /// The father read the conversation with `child`: every message from
    /// them is read now. Returns how many weren't before.
    pub(super) async fn mark_read(&self, child: ChildId) -> Result<usize> {
        let changed = self
            .conn
            .execute(
                "UPDATE message SET read_at = ?2 \
                 WHERE child_id = ?1 AND sender = 'child' AND read_at IS NULL",
                (child.to_string(), Timestamp::now().as_millisecond()),
            )
            .await?;
        Ok(changed as usize)
    }

    /// Every conversation that has a message: its newest, and how many
    /// from the child the father hasn't read.
    pub(super) async fn conversations(&self) -> Result<HashMap<ChildId, ConversationSummary>> {
        let mut rows = self
            .conn
            .query(
                "SELECT child_id, \
                 sum(CASE WHEN sender = 'child' AND read_at IS NULL THEN 1 ELSE 0 END) \
                 FROM message GROUP BY child_id",
                (),
            )
            .await?;
        let mut unread = Vec::new();
        while let Some(row) = rows.next().await? {
            let count: i64 = row.get(1)?;
            unread.push((parse_id::<ChildId>(&row, 0, "child id")?, count as usize));
        }
        drop(rows);
        let mut conversations = HashMap::new();
        for (child, unread) in unread {
            let mut rows = self
                .conn
                .query(
                    "SELECT id, sender, body, created_at, sent_at, read_at FROM message \
                     WHERE child_id = ?1 ORDER BY created_at DESC, id DESC LIMIT 1",
                    [child.to_string()],
                )
                .await?;
            let row = rows.next().await?.ok_or(Error::NotFound("message"))?;
            let last = message_from_row(&row)?;
            conversations.insert(child, ConversationSummary { last, unread });
        }
        Ok(conversations)
    }

    /// Takes in what `child` wrote to the share: their messages the vault
    /// doesn't have yet, and receipts for the father's.
    ///
    /// Only messages sealed under the share's current key are taken. One
    /// under an older key may come from whoever has the access key that
    /// was replaced, which is why it was replaced; one that won't open was
    /// changed on the way. Neither is the child's to show, and neither
    /// stops the sync: it's left where it is.
    pub(super) async fn receive(
        &mut self,
        child: ChildId,
        messages: Vec<SealedMessage>,
        receipts: Vec<(MessageId, i64)>,
    ) -> Result<Received> {
        let (share_id, epoch, key) = self.share_secrets(child).await?;
        let known = self.message_ids(child).await?;
        let now = Timestamp::now().as_millisecond();
        let mut received = Received::default();
        let tx = self.conn.transaction().await?;
        for message in messages {
            if message.sender != Sender::Child
                || message.epoch != epoch
                || known.contains(&message.id)
            {
                continue;
            }
            let Ok(body) = message.open(&key, &share_id) else {
                continue;
            };
            tx.execute(
                "INSERT INTO message (id, child_id, sender, body, created_at, sent_at) \
                 VALUES (?1, ?2, 'child', ?3, ?4, ?5)",
                (
                    message.id.to_string(),
                    child.to_string(),
                    body,
                    message.created_at,
                    now,
                ),
            )
            .await?;
            received.messages += 1;
        }
        for (id, read_at) in receipts {
            received.receipts += tx
                .execute(
                    "UPDATE message SET read_at = ?3 WHERE id = ?1 AND child_id = ?2 \
                     AND sender = 'father' AND read_at IS NULL",
                    (id.to_string(), child.to_string(), read_at),
                )
                .await? as usize;
        }
        tx.commit().await?;
        Ok(received)
    }

    /// The father's messages in `in_share` reached the cloud with a push:
    /// marks those not marked yet as sent. Returns how many.
    pub(super) async fn messages_sent(
        &self,
        child: ChildId,
        in_share: &HashSet<MessageId>,
        at: Timestamp,
    ) -> Result<usize> {
        let mut rows = self
            .conn
            .query(
                "SELECT id FROM message \
                 WHERE child_id = ?1 AND sender = 'father' AND sent_at IS NULL",
                [child.to_string()],
            )
            .await?;
        let mut sending = Vec::new();
        while let Some(row) = rows.next().await? {
            sending.push(parse_id::<MessageId>(&row, 0, "message id")?);
        }
        drop(rows);
        let mut sent = 0;
        for id in sending.into_iter().filter(|id| in_share.contains(id)) {
            let changed = self
                .conn
                .execute(
                    "UPDATE message SET sent_at = ?2 WHERE id = ?1",
                    (id.to_string(), at.as_millisecond()),
                )
                .await?;
            found(changed, "message")?;
            sent += 1;
        }
        Ok(sent)
    }

    /// The projection's messages (see `share_changes`): every message with
    /// `child` that the share doesn't hold under the current key, sealed;
    /// and receipts for the child's messages the father read.
    pub(super) async fn message_changes(
        &self,
        child: ChildId,
        share_id: &str,
        epoch: i64,
        key: &ContentKey,
        manifest: &Manifest,
        changes: &mut ShareChanges,
    ) -> Result<()> {
        let mut rows = self
            .conn
            .query(
                "SELECT id, sender, body, created_at, read_at FROM message WHERE child_id = ?1 \
                 ORDER BY created_at, id",
                [child.to_string()],
            )
            .await?;
        while let Some(row) = rows.next().await? {
            let id: MessageId = parse_id(&row, 0, "message id")?;
            let sender: Sender = row.get::<String>(1)?.parse()?;
            let read_at: Option<i64> = row.get(4)?;
            if sender == Sender::Child
                && let Some(read_at) = read_at
                && !manifest.receipts.contains_key(&id)
            {
                changes.receipts.push((id, read_at));
            }
            if manifest.messages.get(&id).map(|version| version.epoch) == Some(epoch) {
                continue;
            }
            let body: String = row.get(2)?;
            let created_at: i64 = row.get(3)?;
            changes.messages.push(SealedMessage::seal(
                key, share_id, id, sender, created_at, epoch, &body,
            )?);
        }
        Ok(())
    }

    async fn message_ids(&self, child: ChildId) -> Result<HashSet<MessageId>> {
        let mut rows = self
            .conn
            .query(
                "SELECT id FROM message WHERE child_id = ?1",
                [child.to_string()],
            )
            .await?;
        let mut ids = HashSet::new();
        while let Some(row) = rows.next().await? {
            ids.insert(parse_id(&row, 0, "message id")?);
        }
        Ok(ids)
    }
}

fn message_from_row(row: &Row) -> Result<Message> {
    let sent_at: Option<i64> = row.get(4)?;
    let read_at: Option<i64> = row.get(5)?;
    Ok(Message {
        id: parse_id(row, 0, "message id")?,
        sender: row.get::<String>(1)?.parse()?,
        body: row.get(2)?,
        created_at: timestamp(row, 3)?,
        sent: sent_at.is_some(),
        read_at: read_at
            .map(|millis| {
                Timestamp::from_millisecond(millis).map_err(|_| Error::Corrupt {
                    what: "timestamp",
                    value: millis.to_string(),
                })
            })
            .transpose()?,
    })
}
