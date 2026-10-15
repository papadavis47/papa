//! The vault's side of sharing: which notes a child can see, and the
//! projection that seals them. No network: shares are added as the
//! `Sharing` actor would after the relay made them.

use jiff::Timestamp;
use papa_proto::Secret;

use super::*;
use crate::images::{self, tests::sample};
use crate::share::projection::{Manifest, NoteVersion, ShareChanges, decode_image};
use crate::share::{Aad, BlobKind, ChildKeys, NotePayload};
use crate::{Account, NoteDraft};

const SHARE_ID: &str = "0199d3c4-0000-7000-8000-00000000abcd";

/// A vault with Emma (shared) and Leo (not), and the keys Emma's child app
/// would derive.
async fn vault() -> (Store, Child, Child, ChildKeys) {
    let store = Store::open_in_memory().await.unwrap();
    let emma = store
        .add_child(ChildDraft {
            name: "Emma".into(),
            color: 0xe0795b,
        })
        .await
        .unwrap();
    let leo = store
        .add_child(ChildDraft {
            name: "Leo".into(),
            color: 0x5b9be0,
        })
        .await
        .unwrap();
    let keys = share_with(&store, emma.id).await;
    (store, emma, leo, keys)
}

/// Turns sharing on for `child`, as `Sharing::start_sharing` does once the
/// relay made the share.
async fn share_with(store: &Store, child: ChildId) -> ChildKeys {
    let access_key = AccessKey::generate().unwrap();
    let keys = ChildKeys::derive(&access_key).unwrap();
    let content_key = ContentKey::generate().unwrap();
    let share = ChildShare {
        child_id: child,
        share_id: SHARE_ID.into(),
        relay_url: "http://relay.test".into(),
        database_url: "http://sync.test/db/share-1".into(),
        wrapped_key: content_key.wrap(&keys.enc, SHARE_ID, 1).unwrap(),
        access_key,
        key_epoch: 1,
        auth_key_stale: false,
        created_at: Timestamp::now(),
        last_synced_at: None,
    };
    store.add_share(share, content_key).await.unwrap();
    keys
}

async fn note_about(store: &Store, title: &str, children: Vec<ChildId>) -> Note {
    store
        .add_note(NoteDraft {
            title: title.into(),
            body: format!("All about {title}."),
            children,
            labels: Vec::new(),
        })
        .await
        .unwrap()
}

/// What a share database holds after `changes` are applied to `manifest`.
fn applied(mut manifest: Manifest, changes: &ShareChanges) -> Manifest {
    for note in &changes.notes {
        manifest.notes.insert(note.id, note.version);
    }
    for id in &changes.removed_notes {
        manifest.notes.remove(id);
    }
    for blob in &changes.blobs {
        manifest.blobs.insert((blob.image, blob.kind), blob.epoch);
    }
    for blob in &changes.removed_blobs {
        manifest.blobs.remove(blob);
    }
    manifest
}

/// Opens a sealed note the way the child's app will: unwrap the content
/// key with the keys from the access key, then open the note.
fn open_note(
    changes: &ShareChanges,
    id: NoteId,
    keys: &ChildKeys,
    wrapped: &Sealed,
) -> NotePayload {
    let note = changes.notes.iter().find(|n| n.id == id).unwrap();
    let epoch = note.version.epoch;
    let key = ContentKey::unwrap(wrapped, &keys.enc, SHARE_ID, epoch).unwrap();
    let id = id.to_string();
    let aad = Aad::Note {
        share: SHARE_ID,
        note: &id,
        epoch,
    };
    let json = key.open(aad, &note.sealed, "a note").unwrap();
    serde_json::from_slice(&json).unwrap()
}

#[tokio::test]
async fn the_account_is_remembered_and_forgotten() {
    let store = Store::open_in_memory().await.unwrap();
    assert_eq!(store.account().await.unwrap(), None);

    let account = Account {
        relay_url: "https://relay.example".into(),
        email: "dad@example.com".into(),
        session: Secret::new("session-1"),
        expires_at: Timestamp::from_millisecond(1_800_000_000_000).unwrap(),
    };
    store.set_account(account.clone()).await.unwrap();
    assert_eq!(store.account().await.unwrap(), Some(account.clone()));

    let again = Account {
        session: Secret::new("session-2"),
        ..account
    };
    store.set_account(again.clone()).await.unwrap();
    assert_eq!(
        store.account().await.unwrap(),
        Some(again),
        "replaced, not added"
    );

    store.clear_account().await.unwrap();
    assert_eq!(store.account().await.unwrap(), None);
}

#[tokio::test]
async fn a_share_is_stored_with_its_keys() {
    let (store, emma, leo, keys) = vault().await;

    let shares = store.shares().await.unwrap();
    assert_eq!(shares.len(), 1);
    let share = &shares[0];
    assert_eq!(share.child_id, emma.id);
    assert_eq!(share.key_epoch, 1);
    // The stored access key derives the same keys: the father can show it
    // to his child again, and it still works.
    let again = ChildKeys::derive(&share.access_key).unwrap();
    assert_eq!(again.auth.to_secret(), keys.auth.to_secret());
    assert!(ContentKey::unwrap(&share.wrapped_key, &keys.enc, SHARE_ID, 1).is_ok());

    assert!(store.share(leo.id).await.unwrap().is_none());
}

#[tokio::test]
async fn a_note_is_shared_only_with_a_shared_child_it_is_about() {
    let (store, emma, leo, _) = vault().await;
    let about_both = note_about(&store, "Beach", vec![emma.id, leo.id]).await;
    let about_leo = note_about(&store, "Lego", vec![leo.id]).await;

    let shared = store
        .set_note_shared(about_both.id, emma.id, true)
        .await
        .unwrap();
    assert_eq!(shared.shared_with, [emma.id]);
    // Sharing twice is harmless.
    store
        .set_note_shared(about_both.id, emma.id, true)
        .await
        .unwrap();

    // Leo's sharing is off; Emma isn't in "Lego".
    for (note, child) in [(about_both.id, leo.id), (about_leo.id, emma.id)] {
        assert!(matches!(
            store.set_note_shared(note, child, true).await,
            Err(Error::CantShareNote)
        ));
    }
    assert!(
        store
            .note(about_leo.id)
            .await
            .unwrap()
            .shared_with
            .is_empty()
    );

    let unshared = store
        .set_note_shared(about_both.id, emma.id, false)
        .await
        .unwrap();
    assert!(unshared.shared_with.is_empty());
}

#[tokio::test]
async fn unassigning_or_deleting_a_note_unshares_it() {
    let (store, emma, leo, _) = vault().await;
    let note = note_about(&store, "Beach", vec![emma.id, leo.id]).await;
    store.set_note_shared(note.id, emma.id, true).await.unwrap();

    // Still about Emma: still shared.
    let note = store
        .set_note_children(note.id, vec![emma.id])
        .await
        .unwrap();
    assert_eq!(note.shared_with, [emma.id]);
    // No longer about Emma: no longer shared, even if she's added back.
    store
        .set_note_children(note.id, vec![leo.id])
        .await
        .unwrap();
    let note = store
        .set_note_children(note.id, vec![leo.id, emma.id])
        .await
        .unwrap();
    assert!(note.shared_with.is_empty());

    store.set_note_shared(note.id, emma.id, true).await.unwrap();
    store.delete_note(note.id).await.unwrap();
    let changes = store
        .share_changes(emma.id, Manifest::default(), usize::MAX)
        .await
        .unwrap();
    assert!(changes.is_empty(), "{changes:?}");
}

#[tokio::test]
async fn a_shared_child_cant_be_deleted_until_sharing_stops() {
    let (store, emma, _, _) = vault().await;
    let note = note_about(&store, "Beach", vec![emma.id]).await;
    store.set_note_shared(note.id, emma.id, true).await.unwrap();

    assert!(matches!(
        store.delete_child(emma.id).await,
        Err(Error::ChildIsShared(name)) if name == "Emma"
    ));

    store.remove_share(emma.id).await.unwrap();
    assert!(store.note(note.id).await.unwrap().shared_with.is_empty());
    store.delete_child(emma.id).await.unwrap();
}

#[tokio::test]
async fn the_projection_seals_exactly_the_shared_notes() {
    let (store, emma, leo, keys) = vault().await;
    let shared = note_about(&store, "First bike ride", vec![emma.id]).await;
    let private = note_about(&store, "Emma's tantrum", vec![emma.id]).await;
    let leos = note_about(&store, "Lego", vec![leo.id]).await;
    store
        .set_note_shared(shared.id, emma.id, true)
        .await
        .unwrap();
    let label = store.add_label("milestone").await.unwrap();
    store
        .set_note_labels(shared.id, vec![label.id])
        .await
        .unwrap();
    let shared = store.note(shared.id).await.unwrap();

    let changes = store
        .share_changes(emma.id, Manifest::default(), usize::MAX)
        .await
        .unwrap();

    let ids: Vec<NoteId> = changes.notes.iter().map(|n| n.id).collect();
    assert_eq!(ids, [shared.id]);
    assert!(!ids.contains(&private.id) && !ids.contains(&leos.id));
    let wrapped = store.share(emma.id).await.unwrap().unwrap().wrapped_key;
    let payload = open_note(&changes, shared.id, &keys, &wrapped);
    assert_eq!(
        payload,
        NotePayload {
            title: "First bike ride".into(),
            body: "All about First bike ride.".into(),
            created_at: shared.created_at.as_millisecond(),
            updated_at: shared.updated_at.as_millisecond(),
            images: Vec::new(),
        }
    );
    let json = String::from_utf8_lossy(&changes.notes[0].sealed.ciphertext).into_owned();
    assert!(!json.contains("bike") && !json.contains("milestone"));
    assert_eq!(
        changes.notes[0].version,
        NoteVersion {
            updated_at: shared.updated_at.as_millisecond(),
            epoch: 1
        }
    );
}

#[tokio::test]
async fn the_projection_is_idempotent_and_sends_only_what_changed() {
    let (store, emma, _, keys) = vault().await;
    let a = note_about(&store, "A", vec![emma.id]).await;
    let b = note_about(&store, "B", vec![emma.id]).await;
    for note in [a.id, b.id] {
        store.set_note_shared(note, emma.id, true).await.unwrap();
    }
    let first = store
        .share_changes(emma.id, Manifest::default(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(first.notes.len(), 2);
    let manifest = applied(Manifest::default(), &first);

    // Nothing changed: nothing to send.
    let again = store
        .share_changes(emma.id, manifest.clone(), usize::MAX)
        .await
        .unwrap();
    assert!(again.is_empty(), "{again:?}");

    // One note edited, the other unshared.
    tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    store.update_note_text(a.id, "A2", "New").await.unwrap();
    store.set_note_shared(b.id, emma.id, false).await.unwrap();
    let changes = store
        .share_changes(emma.id, manifest.clone(), usize::MAX)
        .await
        .unwrap();
    let ids: Vec<NoteId> = changes.notes.iter().map(|n| n.id).collect();
    assert_eq!(ids, [a.id]);
    assert_eq!(changes.removed_notes, [b.id]);
    let wrapped = store.share(emma.id).await.unwrap().unwrap().wrapped_key;
    assert_eq!(open_note(&changes, a.id, &keys, &wrapped).title, "A2");

    let manifest = applied(manifest, &changes);
    let settled = store
        .share_changes(emma.id, manifest, usize::MAX)
        .await
        .unwrap();
    assert!(settled.is_empty(), "{settled:?}");
}

#[tokio::test]
async fn a_new_key_reseals_everything_under_the_new_epoch() {
    let (store, emma, _, old_keys) = vault().await;
    let note = note_about(&store, "A", vec![emma.id]).await;
    store.set_note_shared(note.id, emma.id, true).await.unwrap();
    let first = store
        .share_changes(emma.id, Manifest::default(), usize::MAX)
        .await
        .unwrap();
    let manifest = applied(Manifest::default(), &first);

    let access_key = AccessKey::generate().unwrap();
    let new_keys = ChildKeys::derive(&access_key).unwrap();
    let content_key = ContentKey::generate().unwrap();
    let rekeyed = store
        .rekey_share(Rekey {
            child: emma.id,
            epoch: 2,
            wrapped: content_key.wrap(&new_keys.enc, SHARE_ID, 2).unwrap(),
            access_key,
            content_key,
        })
        .await
        .unwrap();
    assert_eq!(rekeyed.key_epoch, 2);
    assert!(rekeyed.auth_key_stale, "the relay hasn't heard yet");

    let changes = store
        .share_changes(emma.id, manifest, usize::MAX)
        .await
        .unwrap();
    assert_eq!(changes.notes.len(), 1);
    assert_eq!(changes.notes[0].version.epoch, 2);
    assert_eq!(
        open_note(&changes, note.id, &new_keys, &rekeyed.wrapped_key).title,
        "A"
    );
    // The old access key can't unwrap the new content key.
    assert!(ContentKey::unwrap(&rekeyed.wrapped_key, &old_keys.enc, SHARE_ID, 2).is_err());

    // Rekeying from a stale epoch is refused, so two can't race.
    let content_key = ContentKey::generate().unwrap();
    let stale = store
        .rekey_share(Rekey {
            child: emma.id,
            epoch: 2,
            wrapped: content_key.wrap(&new_keys.enc, SHARE_ID, 2).unwrap(),
            access_key: AccessKey::generate().unwrap(),
            content_key,
        })
        .await;
    assert!(matches!(stale, Err(Error::NotFound("share"))));

    store.auth_key_updated(emma.id, 2).await.unwrap();
    assert!(!store.share(emma.id).await.unwrap().unwrap().auth_key_stale);
}

#[tokio::test]
async fn images_go_as_thumbnail_and_display_copy_in_batches() {
    let (store, emma, _, keys) = vault().await;
    let note = note_about(&store, "Photos", vec![emma.id]).await;
    let mut thumbs = Vec::new();
    for size in [3000, 600] {
        let prepared = images::prepare(sample(image::ImageFormat::Png, size, size / 2)).unwrap();
        let image = store.add_image(note.id, prepared).await.unwrap();
        thumbs.push((image.id, image.thumb));
    }
    store.set_note_shared(note.id, emma.id, true).await.unwrap();

    // A budget of one byte: one image version per batch.
    let mut manifest = Manifest::default();
    let mut batches = Vec::new();
    loop {
        let changes = store
            .share_changes(emma.id, manifest.clone(), 1)
            .await
            .unwrap();
        manifest = applied(manifest, &changes);
        batches.push(changes);
        if !batches.last().unwrap().more {
            break;
        }
    }
    let blobs: Vec<_> = batches.iter().map(|b| b.blobs.len()).collect();
    assert_eq!(blobs, [1, 1, 1, 1], "four versions, one per batch");
    assert_eq!(manifest.blobs.len(), 4);

    // The first note payload lists both images, in order.
    let wrapped = store.share(emma.id).await.unwrap().unwrap().wrapped_key;
    let payload = open_note(&batches[0], note.id, &keys, &wrapped);
    let ids: Vec<String> = thumbs.iter().map(|(id, _)| id.to_string()).collect();
    assert_eq!(payload.images, ids);

    // Each version opens to the vault's bytes: the thumbnail, and a display
    // copy no larger than 2048 px (the 3000 px original stays home).
    let content = ContentKey::unwrap(&wrapped, &keys.enc, SHARE_ID, 1).unwrap();
    for blob in batches.iter().flat_map(|b| &b.blobs) {
        let image = blob.image.to_string();
        let aad = Aad::Blob {
            share: SHARE_ID,
            image: &image,
            kind: blob.kind.as_str(),
            epoch: 1,
        };
        let opened = decode_image(content.open(aad, &blob.sealed, "an image").unwrap()).unwrap();
        let expected = match blob.kind {
            BlobKind::Thumb => thumbs
                .iter()
                .find(|(id, _)| *id == blob.image)
                .unwrap()
                .1
                .clone(),
            BlobKind::Display => store.display_image(blob.image).await.unwrap(),
        };
        assert_eq!(opened, expected);
        if blob.kind == BlobKind::Display {
            let decoded = image::load_from_memory(&opened.bytes).unwrap();
            assert!(decoded.width().max(decoded.height()) <= 2048);
        }
    }

    // Deleting an image removes both its versions.
    store.delete_image(thumbs[0].0).await.unwrap();
    let changes = store
        .share_changes(emma.id, manifest, usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        changes.removed_blobs,
        [
            (thumbs[0].0, BlobKind::Thumb),
            (thumbs[0].0, BlobKind::Display)
        ]
    );
    assert_eq!(changes.notes.len(), 1, "the note's image list changed");
}
