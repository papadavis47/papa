//! Messages end to end: Dad's vault and `Sharing` on one side, the child's
//! `ChildSession` on the other, a real relay and Turso's sync server in
//! between.
//!
//! See `common` for what the tests need installed.

mod common;

use std::path::PathBuf;

use common::{EMAIL, World};
use papa_core::share::KeyCard;
use papa_core::{ChildId, ChildSession, Error, Message, MessageId, Sender, ShareContents};

impl World {
    fn child_dir(&self) -> PathBuf {
        self.root.join("child")
    }

    /// Emma, shared, synced once, and signed in on her own app.
    async fn emma(&self) -> (ChildId, ChildSession, ShareContents) {
        let emma = self.child("Emma").await;
        self.sharing.start_sharing(emma).await.unwrap();
        self.sharing.sync(emma).await.unwrap();
        let (session, contents) = self.sign_in(emma).await;
        (emma, session, contents)
    }

    async fn sign_in(&self, child: ChildId) -> (ChildSession, ShareContents) {
        let share = self.store.share(child).await.unwrap().unwrap();
        let card: KeyCard = KeyCard::new(share.access_key, &share.relay_url)
            .to_string()
            .parse()
            .unwrap();
        let session = ChildSession::start(self.child_dir()).unwrap();
        let contents = session.sign_in(EMAIL.into(), card).await.unwrap();
        (session, contents)
    }

    async fn dad_writes(&self, child: ChildId, body: &str) -> Message {
        self.store
            .add_message(child, MessageId::new(), body)
            .await
            .unwrap()
    }
}

fn bodies(messages: &[Message]) -> Vec<(Sender, &str)> {
    messages
        .iter()
        .map(|message| (message.sender, message.body.as_str()))
        .collect()
}

#[tokio::test]
async fn dad_and_his_child_write_both_ways_with_receipts() {
    let Some(world) = World::new().await else {
        return;
    };
    let (emma, session, contents) = world.emma().await;
    assert!(contents.can_message);
    assert_eq!(contents.messages, []);

    // Dad writes: it's in his vault at once, and sent with the next sync.
    let hello = world.dad_writes(emma, "Proud of you today.").await;
    assert!(!hello.sent);
    let report = world.sharing.sync(emma).await.unwrap();
    assert_eq!(report.messages_sent, 1);
    let dads = world.store.messages(emma).await.unwrap();
    assert!(dads[0].sent);
    assert_eq!(dads[0].read_at, None);

    let contents = session.pull().await.unwrap().expect("a new message");
    assert_eq!(
        bodies(&contents.messages),
        [(Sender::Father, "Proud of you today.")]
    );
    assert_eq!(contents.messages[0].id, hello.id);
    assert_eq!(contents.messages[0].read_at, None);

    // Emma reads it and answers.
    assert_eq!(session.mark_read().await.unwrap(), 1);
    assert_eq!(
        session.mark_read().await.unwrap(),
        0,
        "a message is read once"
    );
    let reply = session
        .send(MessageId::new(), "  Thanks Dad!\n".into())
        .await
        .unwrap();
    assert!(reply.sent);
    assert_eq!(reply.body, "Thanks Dad!");

    // Dad's next sync brings both: her message, unread, and her receipt.
    let report = world.sharing.sync(emma).await.unwrap();
    assert_eq!((report.messages_received, report.receipts), (1, 1));
    let dads = world.store.messages(emma).await.unwrap();
    assert_eq!(
        bodies(&dads),
        [
            (Sender::Father, "Proud of you today."),
            (Sender::Child, "Thanks Dad!")
        ]
    );
    assert!(dads[0].read_at.is_some(), "Emma's receipt");
    assert_eq!(dads[1].id, reply.id);
    assert_eq!(dads[1].created_at, reply.created_at);
    let conversations = world.store.conversations().await.unwrap();
    assert_eq!(conversations[&emma].unread, 1);
    assert_eq!(conversations[&emma].last.body, "Thanks Dad!");

    // Dad reads it; his receipt goes out with the sync after.
    assert_eq!(world.store.mark_read(emma).await.unwrap(), 1);
    assert_eq!(world.store.conversations().await.unwrap()[&emma].unread, 0);
    world.sharing.sync(emma).await.unwrap();
    let contents = session.pull().await.unwrap().expect("a receipt");
    let her_reply = &contents.messages[1];
    assert!(her_reply.sent);
    assert!(her_reply.read_at.is_some());

    // Syncing again is a no-op on both sides.
    let report = world.sharing.sync(emma).await.unwrap();
    assert!(report.is_empty(), "{report:?}");
    assert_eq!(session.pull().await.unwrap(), None);
}

#[tokio::test]
async fn a_message_written_offline_waits_and_goes_with_the_next_pull() {
    let Some(mut world) = World::new().await else {
        return;
    };
    let (emma, session, _) = world.emma().await;

    world.sync.stop();
    let id = MessageId::new();
    let offline = session
        .send(id, "Written on the plane".into())
        .await
        .unwrap();
    assert!(!offline.sent);

    // The app quits and starts again, still offline: the message is
    // there, and still not sent.
    drop(session);
    let session = ChildSession::start(world.child_dir()).unwrap();
    let contents = session.open().await.unwrap().expect("signed in");
    assert_eq!(
        bodies(&contents.messages),
        [(Sender::Child, "Written on the plane")]
    );
    assert!(!contents.messages[0].sent);

    // Back online, the next pull sends it first.
    world.sync.restart();
    let contents = session.pull().await.unwrap().expect("it was sent");
    assert!(contents.messages[0].sent);
    world.sharing.sync(emma).await.unwrap();
    let dads = world.store.messages(emma).await.unwrap();
    assert_eq!(bodies(&dads), [(Sender::Child, "Written on the plane")]);
    assert_eq!(dads[0].id, id);
}

#[tokio::test]
async fn a_new_key_reseals_the_conversation_and_shuts_out_the_old_one() {
    let Some(world) = World::new().await else {
        return;
    };
    let (emma, old_session, _) = world.emma().await;
    world.dad_writes(emma, "Before the new key").await;
    world.sharing.sync(emma).await.unwrap();
    old_session.pull().await.unwrap();
    // Sent under the old key, and not yet seen by Dad's app when he makes
    // the new one: making it fetches messages first, so this isn't lost.
    old_session
        .send(MessageId::new(), "Me too".into())
        .await
        .unwrap();

    world.sharing.new_key(emma).await.unwrap();
    world.sharing.sync(emma).await.unwrap();
    // Whoever still has the old key, and a token from before, can still
    // write a row; Dad's app won't take a message under the old key.
    let _ = old_session.send(MessageId::new(), "Not Emma".into()).await;
    drop(old_session);
    world.sharing.sync(emma).await.unwrap();
    let dads = world.store.messages(emma).await.unwrap();
    assert_eq!(
        bodies(&dads),
        [
            (Sender::Father, "Before the new key"),
            (Sender::Child, "Me too")
        ]
    );

    // With the new key, Emma reads the whole conversation, resealed.
    let (session, contents) = world.sign_in(emma).await;
    assert_eq!(
        bodies(&contents.messages),
        [
            (Sender::Father, "Before the new key"),
            (Sender::Child, "Me too")
        ]
    );
    session
        .send(MessageId::new(), "New key works".into())
        .await
        .unwrap();
    let report = world.sharing.sync(emma).await.unwrap();
    assert_eq!(report.messages_received, 1);
}

#[tokio::test]
async fn messages_need_sharing_on_and_survive_it_stopping() {
    let Some(world) = World::new().await else {
        return;
    };
    let leo = world.child("Leo").await;
    assert!(matches!(
        world.store.add_message(leo, MessageId::new(), "Hi").await,
        Err(Error::NotShared(name)) if name == "Leo"
    ));

    let (emma, session, _) = world.emma().await;
    assert!(matches!(
        session.send(MessageId::new(), " \n".into()).await,
        Err(Error::EmptyMessage)
    ));
    session
        .send(MessageId::new(), "Hi Dad".into())
        .await
        .unwrap();
    world.sharing.sync(emma).await.unwrap();

    // The conversation is Dad's to keep: stopping sharing deletes the
    // cloud copy, not his vault's.
    world.sharing.stop_sharing(emma).await.unwrap();
    let dads = world.store.messages(emma).await.unwrap();
    assert_eq!(bodies(&dads), [(Sender::Child, "Hi Dad")]);
}
