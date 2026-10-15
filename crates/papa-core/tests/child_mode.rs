//! The child's side end to end: `ChildSession`, the code the child's app
//! runs, against the father's `Sharing`, a real relay and Turso's sync
//! server. `sharing.rs` checks the father's side with a child written by
//! hand; these check that the app's own child reads what he shares.
//!
//! See `common` for what the tests need installed.

mod common;

use std::path::{Path, PathBuf};

use common::{EMAIL, World, sample_png};
use papa_core::images::prepare;
use papa_core::share::KeyCard;
use papa_core::share::projection::BlobKind;
use papa_core::{ChildId, ChildSession, Error, NoteDraft, NoteId, SharedNotes};
use papa_proto::ErrorCode;

impl World {
    /// The child's folder, where `ChildSession` keeps its files.
    fn child_dir(&self) -> PathBuf {
        self.root.join("child")
    }

    /// The key card Dad's dialog shows for `child`, as text.
    async fn key_card(&self, child: ChildId) -> String {
        let share = self.store.share(child).await.unwrap().unwrap();
        KeyCard::new(share.access_key, &share.relay_url).to_string()
    }

    /// Signs in as the child, with the key as Dad would hand it over.
    async fn sign_in(&self, child: ChildId) -> (ChildSession, SharedNotes) {
        let session = ChildSession::start(self.child_dir()).unwrap();
        let card = self.key_card(child).await.parse().unwrap();
        let notes = session.sign_in(EMAIL.into(), card).await.unwrap();
        (session, notes)
    }

    async fn note(&self, title: &str, child: ChildId, shared: bool) -> NoteId {
        let note = self
            .store
            .add_note(NoteDraft {
                title: title.into(),
                body: format!("About {title}."),
                children: vec![child],
                labels: Vec::new(),
            })
            .await
            .unwrap();
        if shared {
            self.store
                .set_note_shared(note.id, child, true)
                .await
                .unwrap();
        }
        note.id
    }
}

fn titles(notes: &SharedNotes) -> Vec<&str> {
    notes.notes.iter().map(|note| note.title.as_str()).collect()
}

fn files(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

fn assert_refused<T: std::fmt::Debug>(result: papa_core::Result<T>) {
    match result {
        Err(Error::Relay {
            code: ErrorCode::InvalidCredentials,
            ..
        }) => {}
        other => panic!("expected invalid_credentials, got {other:?}"),
    }
}

#[tokio::test]
async fn a_child_signs_in_with_the_key_card_and_follows_changes() {
    let Some(world) = World::new().await else {
        return;
    };
    let emma = world.child("Emma").await;
    world.sharing.start_sharing(emma).await.unwrap();
    // The test relay isn't Papa's own, so the card names it.
    let card = world.key_card(emma).await;
    assert!(card.ends_with(&format!("@{}", world.relay_url)), "{card}");

    let bike = world.note("First bike ride", emma, true).await;
    let image = world
        .store
        .add_image(bike, prepare(sample_png(2600, 1300)).unwrap())
        .await
        .unwrap();
    world.note("Private", emma, false).await;
    world.sharing.sync(emma).await.unwrap();

    let (session, notes) = world.sign_in(emma).await;
    assert_eq!(notes.father_email, EMAIL);
    assert_eq!(notes.relay_url, world.relay_url);
    assert!(!notes.waiting);
    assert_eq!(titles(&notes), ["First bike ride"]);
    assert_eq!(notes.notes[0].body, "About First bike ride.");
    assert_eq!(notes.notes[0].images, [image.id]);

    // The gallery's thumbnails, and the display copy's size: 2048 wide,
    // the original's 2:1.
    let images = session.note_images(bike).await.unwrap();
    assert_eq!(images.len(), 1);
    assert_eq!(images[0].thumb, image.thumb);
    assert_eq!((images[0].width, images[0].height), (2048, 1024));
    assert_eq!(
        session.image(image.id, BlobKind::Display).await.unwrap(),
        world.store.display_image(image.id).await.unwrap()
    );

    // Nothing new: nothing to show.
    assert_eq!(session.pull().await.unwrap(), None);

    // An edit and a newly shared note arrive with the next pull.
    world
        .store
        .update_note_text(bike, "First bike ride!", "No training wheels.")
        .await
        .unwrap();
    let tooth = world.note("Lost tooth", emma, true).await;
    world.sharing.sync(emma).await.unwrap();
    let notes = session.pull().await.unwrap().expect("something changed");
    assert_eq!(titles(&notes), ["Lost tooth", "First bike ride!"]);

    // Unsharing takes the note away, its images too.
    world
        .store
        .set_note_shared(bike, emma, false)
        .await
        .unwrap();
    world.sharing.sync(emma).await.unwrap();
    let notes = session.pull().await.unwrap().expect("something changed");
    assert_eq!(titles(&notes), ["Lost tooth"]);
    assert_eq!(notes.notes[0].id, tooth);
    assert!(matches!(
        session.image(image.id, BlobKind::Thumb).await,
        Err(Error::NotFound("image"))
    ));
}

#[tokio::test]
async fn the_child_reads_offline_from_the_last_pull() {
    let Some(mut world) = World::new().await else {
        return;
    };
    let emma = world.child("Emma").await;
    world.sharing.start_sharing(emma).await.unwrap();
    world.note("Beach day", emma, true).await;
    world.sharing.sync(emma).await.unwrap();
    let (session, _) = world.sign_in(emma).await;
    drop(session);

    // The app starts again, with the server gone.
    world.sync.stop();
    let session = ChildSession::start(world.child_dir()).unwrap();
    assert!(ChildSession::is_signed_in(&world.child_dir()));
    let notes = session.open().await.unwrap().expect("signed in");
    assert_eq!(titles(&notes), ["Beach day"]);
    assert!(matches!(session.pull().await, Err(Error::Sync(_))));

    // Back online, the next pull brings what changed meanwhile.
    world.sync.restart();
    world.note("Picnic", emma, true).await;
    world.sharing.sync(emma).await.unwrap();
    let notes = session.pull().await.unwrap().expect("something changed");
    assert_eq!(titles(&notes), ["Picnic", "Beach day"]);
}

#[tokio::test]
async fn a_new_key_stops_the_old_one_even_with_a_token_in_hand() {
    let Some(world) = World::new().await else {
        return;
    };
    let emma = world.child("Emma").await;
    world.sharing.start_sharing(emma).await.unwrap();
    world.note("School play", emma, true).await;
    world.sharing.sync(emma).await.unwrap();
    let (old_session, _) = world.sign_in(emma).await;

    world.sharing.new_key(emma).await.unwrap();
    world.sharing.sync(emma).await.unwrap();
    // The old session's token hasn't expired, and the pull brings the
    // resealed rows; the relay is asked again, and refuses the old key.
    assert_refused(old_session.pull().await);
    drop(old_session);

    let (_, notes) = world.sign_in(emma).await;
    assert_eq!(titles(&notes), ["School play"]);
}

#[tokio::test]
async fn a_new_key_used_before_dad_syncs_waits_for_his_sync() {
    let Some(world) = World::new().await else {
        return;
    };
    let emma = world.child("Emma").await;
    world.sharing.start_sharing(emma).await.unwrap();
    world.note("School play", emma, true).await;
    world.sharing.sync(emma).await.unwrap();

    // Dad makes a new key and hands it over before his app syncs.
    world.sharing.new_key(emma).await.unwrap();
    let (session, notes) = world.sign_in(emma).await;
    assert!(notes.waiting, "{notes:?}");
    assert!(notes.notes.is_empty());

    world.sharing.sync(emma).await.unwrap();
    let notes = session.pull().await.unwrap().expect("something changed");
    assert!(!notes.waiting);
    assert_eq!(titles(&notes), ["School play"]);
}

#[tokio::test]
async fn stopping_sharing_tells_the_child_and_signing_out_cleans_up() {
    let Some(world) = World::new().await else {
        return;
    };
    let emma = world.child("Emma").await;
    world.sharing.start_sharing(emma).await.unwrap();
    // Before Dad's first sync there's nothing to read, which is fine.
    let (session, notes) = world.sign_in(emma).await;
    assert_eq!(notes.notes, []);
    assert!(!notes.waiting);
    assert_eq!(files(&world.child_dir()).first().unwrap(), "session.json");

    world.sharing.stop_sharing(emma).await.unwrap();
    // The running app learns at its next token (every 15 minutes); one
    // that starts again learns at its first pull, and keeps what it had
    // until then.
    drop(session);
    let session = ChildSession::start(world.child_dir()).unwrap();
    assert!(session.open().await.unwrap().is_some());
    assert_refused(session.pull().await);

    session.sign_out().await.unwrap();
    assert_eq!(files(&world.child_dir()), Vec::<String>::new());
    assert!(!ChildSession::is_signed_in(&world.child_dir()));
    assert_eq!(session.open().await.unwrap(), None);
}

#[tokio::test]
async fn a_wrong_email_or_key_leaves_nothing_behind() {
    let Some(mut world) = World::new().await else {
        return;
    };
    let emma = world.child("Emma").await;
    world.sharing.start_sharing(emma).await.unwrap();
    let card: KeyCard = world.key_card(emma).await.parse().unwrap();
    let session = ChildSession::start(world.child_dir()).unwrap();

    assert_refused(
        session
            .sign_in("mom@example.com".into(), card.clone())
            .await,
    );
    let stranger = KeyCard::new(
        papa_core::share::AccessKey::generate().unwrap(),
        &card.relay_url,
    );
    assert_refused(session.sign_in(EMAIL.into(), stranger).await);

    // The relay says yes, but the sync server is down: the half-made
    // replica goes too.
    world.sync.stop();
    assert!(matches!(
        session.sign_in(EMAIL.into(), card.clone()).await,
        Err(Error::Sync(_))
    ));
    world.sync.restart();

    assert_eq!(files(&world.child_dir()), Vec::<String>::new());
    assert!(matches!(session.pull().await, Err(Error::ChildSignedOut)));
    // The right ones still work, and the email's case doesn't matter.
    session
        .sign_in(EMAIL.to_uppercase(), card)
        .await
        .expect("signs in");
}

#[tokio::test]
async fn signing_in_with_another_key_replaces_what_was_here() {
    let Some(world) = World::new().await else {
        return;
    };
    let emma = world.child("Emma").await;
    let leo = world.child("Leo").await;
    for child in [emma, leo] {
        world.sharing.start_sharing(child).await.unwrap();
    }
    world.note("For Emma", emma, true).await;
    world.note("For Leo", leo, true).await;
    world.sharing.sync_all().await.unwrap();

    // One computer, first Emma's notes, then Leo's.
    let (emma_session, notes) = world.sign_in(emma).await;
    assert_eq!(titles(&notes), ["For Emma"]);
    drop(emma_session);
    let (leo_session, notes) = world.sign_in(leo).await;
    assert_eq!(titles(&notes), ["For Leo"]);
    assert_eq!(leo_session.pull().await.unwrap(), None);
}
