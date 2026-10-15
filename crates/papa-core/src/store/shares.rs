//! The vault's side of sharing: the father's account, each child's share
//! and its keys, and the projection of shared notes (see
//! [`projection`](crate::share::projection)).

use std::collections::HashSet;

use jiff::Timestamp;
use papa_proto::Secret;
use turso::{Row, Value};

use super::db::{Db, encoded_image, found, parse_id, timestamp};
use crate::share::projection::{
    Manifest, NoteVersion, SealedBlob, SealedNote, ShareChanges, encode_image,
};
use crate::share::{Aad, AccessKey, BlobKind, ContentKey, NotePayload, Sealed};
use crate::{Account, ChildId, ChildShare, Error, ImageId, NoteId, Result};

const SELECT_SHARES: &str = "SELECT child_id, share_id, relay_url, database_url, access_key, \
     key_epoch, wrapped_nonce, wrapped_key, auth_key_stale, created_at, last_synced_at FROM share";

impl Db {
    // --- The account ---

    pub(super) async fn account(&self) -> Result<Option<Account>> {
        let mut rows = self
            .conn
            .query(
                "SELECT relay_url, email, session_token, expires_at FROM account",
                (),
            )
            .await?;
        let Some(row) = rows.next().await? else {
            return Ok(None);
        };
        Ok(Some(Account {
            relay_url: row.get(0)?,
            email: row.get(1)?,
            session: Secret::new(row.get::<String>(2)?),
            expires_at: timestamp(&row, 3)?,
        }))
    }

    /// Signs in: replaces the account, if there was one.
    pub(super) async fn set_account(&self, account: &Account) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO account (id, relay_url, email, session_token, expires_at) \
                 VALUES (1, ?1, ?2, ?3, ?4) ON CONFLICT (id) DO UPDATE SET \
                 relay_url = excluded.relay_url, email = excluded.email, \
                 session_token = excluded.session_token, expires_at = excluded.expires_at",
                (
                    account.relay_url.as_str(),
                    account.email.as_str(),
                    account.session.expose(),
                    account.expires_at.as_millisecond(),
                ),
            )
            .await?;
        Ok(())
    }

    pub(super) async fn clear_account(&self) -> Result<()> {
        self.conn.execute("DELETE FROM account", ()).await?;
        Ok(())
    }

    // --- Shares ---

    /// Every child's share, in sidebar order.
    pub(super) async fn shares(&self) -> Result<Vec<ChildShare>> {
        let mut rows = self
            .conn
            .query(
                &format!(
                    "{SELECT_SHARES} JOIN child ON child.id = share.child_id \
                     ORDER BY child.sort_order, child.id"
                ),
                (),
            )
            .await?;
        let mut shares = Vec::new();
        while let Some(row) = rows.next().await? {
            shares.push(share_from_row(&row)?);
        }
        Ok(shares)
    }

    pub(super) async fn share(&self, child: ChildId) -> Result<Option<ChildShare>> {
        let mut rows = self
            .conn
            .query(
                &format!("{SELECT_SHARES} WHERE child_id = ?1"),
                [child.to_string()],
            )
            .await?;
        rows.next()
            .await?
            .map(|row| share_from_row(&row))
            .transpose()
    }

    /// Remembers a new share, made at the relay a moment ago.
    pub(super) async fn add_share(
        &self,
        share: &ChildShare,
        content_key: &ContentKey,
    ) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO share (child_id, share_id, relay_url, database_url, access_key, \
                 content_key, wrapped_nonce, wrapped_key, key_epoch, auth_key_stale, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                (
                    share.child_id.to_string(),
                    share.share_id.as_str(),
                    share.relay_url.as_str(),
                    share.database_url.as_str(),
                    share.access_key.to_string(),
                    content_key.as_bytes(),
                    share.wrapped_key.nonce.as_slice(),
                    share.wrapped_key.ciphertext.as_slice(),
                    share.key_epoch,
                    i64::from(share.auth_key_stale),
                    share.created_at.as_millisecond(),
                ),
            )
            .await?;
        Ok(())
    }

    /// Replaces a share's access key and content key, as `epoch`, which
    /// must be one more than the current one. The relay hasn't heard of
    /// the new key yet: the share is marked `auth_key_stale` until
    /// [`auth_key_updated`](Self::auth_key_updated).
    pub(super) async fn rekey_share(
        &self,
        child: ChildId,
        epoch: i64,
        access_key: &AccessKey,
        content_key: &ContentKey,
        wrapped: &Sealed,
    ) -> Result<ChildShare> {
        let changed = self
            .conn
            .execute(
                "UPDATE share SET access_key = ?3, content_key = ?4, wrapped_nonce = ?5, \
                 wrapped_key = ?6, key_epoch = ?2, auth_key_stale = 1 \
                 WHERE child_id = ?1 AND key_epoch = ?2 - 1",
                (
                    child.to_string(),
                    epoch,
                    access_key.to_string(),
                    content_key.as_bytes(),
                    wrapped.nonce.as_slice(),
                    wrapped.ciphertext.as_slice(),
                ),
            )
            .await?;
        found(changed, "share")?;
        self.share(child).await?.ok_or(Error::NotFound("share"))
    }

    /// The relay now knows the access key of `epoch`.
    pub(super) async fn auth_key_updated(&self, child: ChildId, epoch: i64) -> Result<()> {
        self.conn
            .execute(
                "UPDATE share SET auth_key_stale = 0 WHERE child_id = ?1 AND key_epoch = ?2",
                (child.to_string(), epoch),
            )
            .await?;
        Ok(())
    }

    pub(super) async fn share_synced(&self, child: ChildId, at: Timestamp) -> Result<()> {
        self.conn
            .execute(
                "UPDATE share SET last_synced_at = ?2 WHERE child_id = ?1",
                (child.to_string(), at.as_millisecond()),
            )
            .await?;
        Ok(())
    }

    /// Forgets a share, and which notes it had (they cascade).
    pub(super) async fn remove_share(&self, child: ChildId) -> Result<()> {
        let changed = self
            .conn
            .execute("DELETE FROM share WHERE child_id = ?1", [child.to_string()])
            .await?;
        found(changed, "share")
    }

    // --- The projection ---

    /// What `manifest`'s database is missing, or has too much of, for
    /// `child`'s share: notes, images and messages sealed with the current
    /// key, and receipts for the messages the father read.
    ///
    /// Images are batched: once those sealed so far add up to `budget`
    /// bytes, the rest wait for the next call and `more` is set. Notes
    /// always come in full; text is small.
    pub(super) async fn share_changes(
        &self,
        child: ChildId,
        manifest: &Manifest,
        budget: usize,
    ) -> Result<ShareChanges> {
        let (share_id, epoch, key) = self.share_secrets(child).await?;
        let mut changes = ShareChanges::default();

        // Notes: the shared ones, with the version each should be at.
        let mut rows = self
            .conn
            .query(
                "SELECT note.id, note.updated_at FROM note \
                 JOIN note_share ON note_share.note_id = note.id AND note_share.child_id = ?1 \
                 WHERE note.deleted_at IS NULL",
                [child.to_string()],
            )
            .await?;
        let mut wanted = Vec::new();
        while let Some(row) = rows.next().await? {
            let id: NoteId = parse_id(&row, 0, "note id")?;
            let updated_at: i64 = row.get(1)?;
            wanted.push((id, updated_at));
        }
        drop(rows);

        let shared: HashSet<NoteId> = wanted.iter().map(|(id, _)| *id).collect();
        for (id, updated_at) in wanted {
            let version = NoteVersion { updated_at, epoch };
            if manifest.notes.get(&id) == Some(&version) {
                continue;
            }
            let payload = self.note_payload(id).await?;
            let json = zeroize::Zeroizing::new(
                serde_json::to_vec(&payload).expect("a note payload serializes"),
            );
            let note = id.to_string();
            let aad = Aad::Note {
                share: &share_id,
                note: &note,
                epoch,
            };
            changes.notes.push(SealedNote {
                id,
                version,
                sealed: key.seal(aad, &json)?,
            });
        }
        changes.removed_notes = manifest
            .notes
            .keys()
            .filter(|id| !shared.contains(id))
            .copied()
            .collect();
        changes.removed_notes.sort();

        // Images of the shared notes, each as a thumbnail and a display copy.
        let mut rows = self
            .conn
            .query(
                "SELECT image.id, image.note_id FROM image \
                 JOIN note_share ON note_share.note_id = image.note_id AND note_share.child_id = ?1 \
                 ORDER BY image.id",
                [child.to_string()],
            )
            .await?;
        let mut images = Vec::new();
        while let Some(row) = rows.next().await? {
            let image: ImageId = parse_id(&row, 0, "image id")?;
            let note: NoteId = parse_id(&row, 1, "note id")?;
            images.push((image, note));
        }
        drop(rows);

        let mut wanted_blobs = HashSet::new();
        let mut used = 0;
        for (image, note) in images {
            for kind in BlobKind::ALL {
                wanted_blobs.insert((image, kind));
                if changes.more || manifest.blobs.get(&(image, kind)) == Some(&epoch) {
                    continue;
                }
                if used >= budget {
                    // Keep going, to learn which blobs are still wanted,
                    // but seal no more in this batch.
                    changes.more = true;
                    continue;
                }
                let bytes = encode_image(&self.blob_bytes(image, kind).await?);
                let image_text = image.to_string();
                let aad = Aad::Blob {
                    share: &share_id,
                    image: &image_text,
                    kind: kind.as_str(),
                    epoch,
                };
                let sealed = key.seal(aad, &bytes)?;
                used += sealed.ciphertext.len();
                changes.blobs.push(SealedBlob {
                    image,
                    note,
                    kind,
                    epoch,
                    sealed,
                });
            }
        }
        changes.removed_blobs = manifest
            .blobs
            .keys()
            .filter(|blob| !wanted_blobs.contains(blob))
            .copied()
            .collect();
        changes.removed_blobs.sort();

        self.message_changes(child, &share_id, epoch, &key, manifest, &mut changes)
            .await?;
        Ok(changes)
    }

    /// A share's id, key epoch and content key.
    pub(super) async fn share_secrets(&self, child: ChildId) -> Result<(String, i64, ContentKey)> {
        let mut rows = self
            .conn
            .query(
                "SELECT share_id, key_epoch, content_key FROM share WHERE child_id = ?1",
                [child.to_string()],
            )
            .await?;
        let row = rows.next().await?.ok_or(Error::NotFound("share"))?;
        let key = ContentKey::from_bytes(&blob(&row, 2, "content key")?)?;
        Ok((row.get(0)?, row.get(1)?, key))
    }

    /// A note as the child will see it.
    async fn note_payload(&self, id: NoteId) -> Result<NotePayload> {
        let mut rows = self
            .conn
            .query(
                "SELECT title, body_md, created_at, updated_at FROM note WHERE id = ?1",
                [id.to_string()],
            )
            .await?;
        let row = rows.next().await?.ok_or(Error::NotFound("note"))?;
        drop(rows);
        let mut payload = NotePayload {
            title: row.get(0)?,
            body: row.get(1)?,
            created_at: row.get(2)?,
            updated_at: row.get(3)?,
            images: Vec::new(),
        };
        let mut rows = self
            .conn
            .query(
                "SELECT id FROM image WHERE note_id = ?1 ORDER BY id",
                [id.to_string()],
            )
            .await?;
        while let Some(row) = rows.next().await? {
            payload.images.push(row.get(0)?);
        }
        Ok(payload)
    }

    /// The bytes a blob row carries: the thumbnail, or the display copy
    /// (the original when it needed none, being small and upright already).
    async fn blob_bytes(&self, image: ImageId, kind: BlobKind) -> Result<crate::EncodedImage> {
        match kind {
            BlobKind::Display => self.display_image(image).await,
            BlobKind::Thumb => {
                let mut rows = self
                    .conn
                    .query(
                        "SELECT mime, bytes FROM image_data WHERE image_id = ?1 AND kind = 'thumb'",
                        [image.to_string()],
                    )
                    .await?;
                let row = rows.next().await?.ok_or(Error::NotFound("image"))?;
                encoded_image(&row, 0)
            }
        }
    }
}

fn share_from_row(row: &Row) -> Result<ChildShare> {
    let access_key: String = row.get(4)?;
    let stale: i64 = row.get(8)?;
    let last_synced: Option<i64> = row.get(10)?;
    Ok(ChildShare {
        child_id: parse_id(row, 0, "child id")?,
        share_id: row.get(1)?,
        relay_url: row.get(2)?,
        database_url: row.get(3)?,
        access_key: access_key.parse().map_err(|_| Error::Corrupt {
            what: "access key",
            value: "not an access key".into(),
        })?,
        key_epoch: row.get(5)?,
        wrapped_key: Sealed::from_parts(
            &blob(row, 6, "wrapped key")?,
            blob(row, 7, "wrapped key")?,
        )
        .map_err(|_| Error::Corrupt {
            what: "wrapped key",
            value: "a nonce of the wrong length".into(),
        })?,
        auth_key_stale: stale != 0,
        created_at: timestamp(row, 9)?,
        last_synced_at: last_synced
            .map(|millis| {
                Timestamp::from_millisecond(millis).map_err(|_| Error::Corrupt {
                    what: "timestamp",
                    value: millis.to_string(),
                })
            })
            .transpose()?,
    })
}

/// Reads column `idx` as bytes.
fn blob(row: &Row, idx: usize, what: &'static str) -> Result<Vec<u8>> {
    match row.get_value(idx)? {
        Value::Blob(bytes) => Ok(bytes),
        _ => Err(Error::Corrupt {
            what,
            value: "something other than bytes".into(),
        }),
    }
}
