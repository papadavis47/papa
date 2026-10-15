//! Sharing end to end: a real relay (in-process, local mode) and Turso's
//! own sync server (`tursodb --sync-server`), with the father's side driven
//! through `Sharing` and the child's side done by hand, the way Phase 12's
//! app will: the relay client, a fresh Turso replica, and the keys from
//! the access key. Nothing on the child's side uses the father's code
//! paths, so the two can't agree on a mistake.
//!
//! Needs `tursodb` 0.8.2 on the `PATH`, or in `$TURSODB`. Without it the
//! tests say so and pass, unless `PAPA_REQUIRE_TURSODB` is set (CI sets it).

use std::collections::BTreeMap;
use std::net::TcpListener as StdListener;
use std::path::{Path, PathBuf};
use std::process::{Child as Process, Command, Stdio};
use std::time::{Duration, Instant};

use papa_core::images::prepare;
use papa_core::relay::RelayClient;
use papa_core::share::projection::decode_image;
use papa_core::share::{Aad, AccessKey, ChildKeys, ContentKey, NotePayload, Sealed};
use papa_core::{ChildDraft, ChildId, ChildShare, EncodedImage, Error, NoteDraft, Sharing, Store};
use papa_proto::{ErrorCode, Secret};
use papa_relay::provision::LocalSync;
use papa_relay::rate_limit::RateLimiter;
use papa_relay::{AppState, Provisioner, Shared, now_ms, secrets};
use tempfile::TempDir;
use turso::Value;

const EMAIL: &str = "dad@example.com";
const DAY_MS: i64 = 24 * 60 * 60 * 1000;

/// `tursodb --sync-server` on a free port, serving databases from a folder.
struct SyncServer {
    process: Process,
    port: u16,
    dir: PathBuf,
    bin: PathBuf,
}

impl SyncServer {
    fn start(bin: PathBuf, dir: PathBuf, port: u16) -> Self {
        let process = Command::new(&bin)
            .arg("--sync-server")
            .arg(format!("127.0.0.1:{port}"))
            .arg("--sync-dir")
            .arg(&dir)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("couldn't start tursodb");
        let started = Instant::now();
        while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
            assert!(
                started.elapsed() < Duration::from_secs(20),
                "tursodb didn't start"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        Self {
            process,
            port,
            dir,
            bin,
        }
    }

    fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    fn stop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }

    fn restart(&mut self) {
        self.stop();
        *self = Self::start(self.bin.clone(), self.dir.clone(), self.port);
    }
}

impl Drop for SyncServer {
    fn drop(&mut self) {
        self.stop();
    }
}

fn tursodb() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("TURSODB") {
        return Some(path.into());
    }
    let found = std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join("tursodb"))
            .find(|path| path.is_file())
    });
    if found.is_none() {
        assert!(
            std::env::var_os("PAPA_REQUIRE_TURSODB").is_none(),
            "tursodb isn't installed, and PAPA_REQUIRE_TURSODB is set"
        );
        eprintln!("skipping: tursodb isn't on the PATH (see the README)");
    }
    found
}

fn free_port() -> u16 {
    StdListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Everything a test needs: the sync server, a relay, a vault and the
/// father's side of sharing, signed in.
struct World {
    root: PathBuf,
    sync: SyncServer,
    relay_url: String,
    relay_state: AppState,
    store: Store,
    sharing: Sharing,
    shares_dir: PathBuf,
    /// Last, so it's deleted after the sync server has stopped.
    _dir: TempDir,
}

impl World {
    async fn new() -> Option<Self> {
        let bin = tursodb()?;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_owned();
        let sync_dir = root.join("sync");
        std::fs::create_dir(&sync_dir).unwrap();
        let sync = SyncServer::start(bin, sync_dir.clone(), free_port());

        let relay_state = AppState::new(Shared {
            store: papa_relay::Store::in_memory().await.unwrap(),
            provisioner: Provisioner::Local(LocalSync::new(&sync.url(), Some(sync_dir))),
            rate_limiter: RateLimiter::new(1000),
            client_ip_header: None,
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let relay_url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(papa_relay::serve(
            listener,
            relay_state.clone(),
            std::future::pending(),
        ));

        let store = Store::open_in_memory().await.unwrap();
        let shares_dir = root.join("shares");
        let sharing = Sharing::start(store.clone(), shares_dir.clone()).unwrap();
        let world = Self {
            _dir: dir,
            root,
            sync,
            relay_url,
            relay_state,
            store,
            sharing,
            shares_dir,
        };
        let invite = world.invite().await;
        world
            .sharing
            .sign_up(
                world.relay_url.clone(),
                Secret::new(invite),
                EMAIL.into(),
                Secret::new("correct horse battery"),
            )
            .await
            .unwrap();
        Some(world)
    }

    async fn invite(&self) -> String {
        let code = secrets::new_invite_code().unwrap();
        let hash = secrets::sha256(secrets::normalize_invite_code(&code).as_bytes());
        let now = now_ms();
        self.relay_state
            .store
            .add_invite(&hash, "test", now, now + DAY_MS)
            .await
            .unwrap();
        code
    }

    async fn child(&self, name: &str) -> ChildId {
        let draft = ChildDraft {
            name: name.into(),
            color: 0xe0795b,
        };
        self.store.add_child(draft).await.unwrap().id
    }

    fn relay(&self) -> RelayClient {
        RelayClient::new(&self.relay_url).unwrap()
    }

    /// What the child sees, signing in with Dad's email and `access_key`.
    async fn child_view(&self, access_key: &AccessKey) -> papa_core::Result<ChildView> {
        let keys = ChildKeys::derive(access_key)?;
        let token = self.relay().child_token(EMAIL, &keys.auth).await?;
        // A new replica each time: it starts from what the cloud has.
        let path = self.root.join(format!("child-{}.db", now_ms()));
        let db = turso::sync::Builder::new_remote(path.to_str().unwrap())
            .with_remote_url(&token.database.database_url)
            .with_auth_token(token.database.token.expose())
            .build()
            .await?;
        let conn = db.connect().await?;
        read_share(&conn, &keys, &token.share_id).await
    }
}

/// The child's view of a share: its notes by title, and its images.
#[derive(Debug)]
struct ChildView {
    epoch: i64,
    notes: BTreeMap<String, NotePayload>,
    /// (image id, kind) → bytes.
    images: BTreeMap<(String, String), EncodedImage>,
    /// Rows sealed under an older epoch than the share's key: unreadable
    /// for now, until the father's next sync reseals them.
    stale_rows: usize,
}

async fn rows(conn: &turso::Connection, sql: &str) -> Vec<Vec<Value>> {
    let mut rows = conn.query(sql, ()).await.unwrap();
    let mut all = Vec::new();
    while let Some(row) = rows.next().await.unwrap() {
        all.push(
            (0..row.column_count())
                .map(|i| row.get_value(i).unwrap())
                .collect(),
        );
    }
    all
}

fn text(value: &Value) -> String {
    match value {
        Value::Text(text) => text.clone(),
        other => panic!("not text: {other:?}"),
    }
}

fn int(value: &Value) -> i64 {
    match value {
        Value::Integer(int) => *int,
        other => panic!("not an integer: {other:?}"),
    }
}

fn sealed(nonce: &Value, ciphertext: &Value) -> Sealed {
    match (nonce, ciphertext) {
        (Value::Blob(nonce), Value::Blob(ciphertext)) => {
            Sealed::from_parts(nonce, ciphertext.clone()).unwrap()
        }
        other => panic!("not blobs: {other:?}"),
    }
}

async fn read_share(
    conn: &turso::Connection,
    keys: &ChildKeys,
    share: &str,
) -> papa_core::Result<ChildView> {
    let key_rows = rows(conn, "SELECT epoch, nonce, wrapped_key FROM share_key").await;
    assert_eq!(key_rows.len(), 1, "one key at a time");
    let epoch = int(&key_rows[0][0]);
    let key = ContentKey::unwrap(
        &sealed(&key_rows[0][1], &key_rows[0][2]),
        &keys.enc,
        share,
        epoch,
    )?;

    let mut view = ChildView {
        epoch,
        notes: BTreeMap::new(),
        images: BTreeMap::new(),
        stale_rows: 0,
    };
    for row in rows(
        conn,
        "SELECT id, key_epoch, nonce, ciphertext FROM shared_note",
    )
    .await
    {
        if int(&row[1]) != epoch {
            view.stale_rows += 1;
            continue;
        }
        let id = text(&row[0]);
        let aad = Aad::Note {
            share,
            note: &id,
            epoch,
        };
        let json = key.open(aad, &sealed(&row[2], &row[3]), "a note")?;
        let payload: NotePayload = serde_json::from_slice(&json).unwrap();
        view.notes.insert(payload.title.clone(), payload);
    }
    for row in rows(
        conn,
        "SELECT id, kind, key_epoch, nonce, ciphertext FROM shared_blob",
    )
    .await
    {
        if int(&row[2]) != epoch {
            view.stale_rows += 1;
            continue;
        }
        let (id, kind) = (text(&row[0]), text(&row[1]));
        let aad = Aad::Blob {
            share,
            image: &id,
            kind: &kind,
            epoch,
        };
        let bytes = key.open(aad, &sealed(&row[3], &row[4]), "an image")?;
        view.images.insert((id, kind), decode_image(bytes)?);
    }
    Ok(view)
}

fn sample_png(width: u32, height: u32) -> Vec<u8> {
    let pixels = image::RgbImage::from_fn(width, height, |x, y| image::Rgb([x as u8, y as u8, 7]));
    let mut bytes = std::io::Cursor::new(Vec::new());
    pixels
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
    bytes.into_inner()
}

fn replica_files(dir: &Path, child: ChildId) -> Vec<String> {
    let prefix = format!("{child}.db");
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .filter(|name| name.starts_with(&prefix))
                .collect()
        })
        .unwrap_or_default()
}

fn assert_relay_refuses(result: papa_core::Result<ChildView>) {
    match result {
        Err(Error::Relay {
            code: ErrorCode::InvalidCredentials,
            ..
        }) => {}
        other => panic!("expected invalid_credentials, got {other:?}"),
    }
}

#[tokio::test]
async fn a_child_reads_exactly_what_was_shared_and_follows_changes() {
    let Some(world) = World::new().await else {
        return;
    };
    let emma = world.child("Emma").await;
    let leo = world.child("Leo").await;
    let share: ChildShare = world.sharing.start_sharing(emma).await.unwrap();

    let bike = world
        .store
        .add_note(NoteDraft {
            title: "First bike ride".into(),
            body: "She refused the training wheels.".into(),
            children: vec![emma, leo],
            labels: Vec::new(),
        })
        .await
        .unwrap();
    let image = world
        .store
        .add_image(bike.id, prepare(sample_png(2600, 1300)).unwrap())
        .await
        .unwrap();
    let private = world
        .store
        .add_note(NoteDraft {
            title: "Private".into(),
            body: "Not for Emma's eyes.".into(),
            children: vec![emma],
            labels: Vec::new(),
        })
        .await
        .unwrap();
    world
        .store
        .set_note_shared(bike.id, emma, true)
        .await
        .unwrap();

    let report = world.sharing.sync(emma).await.unwrap();
    assert_eq!((report.notes, report.images), (1, 2));
    assert!(replica_files(&world.shares_dir, emma).contains(&format!("{emma}.db")));
    assert!(
        world
            .store
            .share(emma)
            .await
            .unwrap()
            .unwrap()
            .last_synced_at
            .is_some()
    );

    let view = world.child_view(&share.access_key).await.unwrap();
    assert_eq!(view.epoch, 1);
    assert_eq!(view.notes.keys().collect::<Vec<_>>(), ["First bike ride"]);
    let note = &view.notes["First bike ride"];
    assert_eq!(note.body, "She refused the training wheels.");
    assert_eq!(note.images, [image.id.to_string()]);
    assert!(!view.notes.contains_key(&private.title));
    let thumb = &view.images[&(image.id.to_string(), "thumb".into())];
    assert_eq!(*thumb, image.thumb);
    let display = &view.images[&(image.id.to_string(), "display".into())];
    assert_eq!(*display, world.store.display_image(image.id).await.unwrap());

    // Edit it, share the other, and the child sees both after a sync.
    world
        .store
        .update_note_text(bike.id, "First bike ride!", "No training wheels.")
        .await
        .unwrap();
    world
        .store
        .set_note_shared(private.id, emma, true)
        .await
        .unwrap();
    let report = world.sharing.sync(emma).await.unwrap();
    assert_eq!((report.notes, report.images), (2, 0));
    let view = world.child_view(&share.access_key).await.unwrap();
    assert_eq!(
        view.notes.keys().collect::<Vec<_>>(),
        ["First bike ride!", "Private"]
    );

    // Unshare one and delete the other: both disappear, images too.
    world
        .store
        .set_note_shared(private.id, emma, false)
        .await
        .unwrap();
    world.store.delete_note(bike.id).await.unwrap();
    let report = world.sharing.sync(emma).await.unwrap();
    assert_eq!((report.removed_notes, report.removed_images), (2, 2));
    let view = world.child_view(&share.access_key).await.unwrap();
    assert!(view.notes.is_empty() && view.images.is_empty(), "{view:?}");

    // Syncing again sends nothing: the projection is idempotent.
    let report = world.sharing.sync(emma).await.unwrap();
    assert!(report.is_empty(), "{report:?}");

    // Leo was never shared: nothing to sync, and no key to sign in with.
    assert!(matches!(
        world.sharing.sync(leo).await,
        Err(Error::NotFound("share"))
    ));
}

#[tokio::test]
async fn a_new_key_locks_out_the_old_one_and_reseals_everything() {
    let Some(world) = World::new().await else {
        return;
    };
    let emma = world.child("Emma").await;
    let old = world.sharing.start_sharing(emma).await.unwrap();
    let note = world
        .store
        .add_note(NoteDraft {
            title: "School play".into(),
            body: "A tree, line two.".into(),
            children: vec![emma],
            labels: Vec::new(),
        })
        .await
        .unwrap();
    world
        .store
        .set_note_shared(note.id, emma, true)
        .await
        .unwrap();
    world.sharing.sync(emma).await.unwrap();
    assert_eq!(
        world.child_view(&old.access_key).await.unwrap().notes.len(),
        1
    );

    let new = world.sharing.new_key(emma).await.unwrap();
    assert_ne!(new.access_key, old.access_key);
    assert_eq!(new.key_epoch, 2);
    assert!(!new.auth_key_stale, "the relay heard about it");

    // The relay refuses the old key at once, before any sync.
    assert_relay_refuses(world.child_view(&old.access_key).await);

    let report = world.sharing.sync(emma).await.unwrap();
    assert_eq!(report.notes, 1, "resealed under the new epoch");
    let view = world.child_view(&new.access_key).await.unwrap();
    assert_eq!((view.epoch, view.stale_rows), (2, 0));
    assert_eq!(view.notes["School play"].body, "A tree, line two.");
}

#[tokio::test]
async fn a_sync_cut_short_finishes_on_the_next_one() {
    let Some(mut world) = World::new().await else {
        return;
    };
    let emma = world.child("Emma").await;
    let share = world.sharing.start_sharing(emma).await.unwrap();
    let note = world
        .store
        .add_note(NoteDraft {
            title: "Lost tooth".into(),
            body: "The first one.".into(),
            children: vec![emma],
            labels: Vec::new(),
        })
        .await
        .unwrap();
    world
        .store
        .set_note_shared(note.id, emma, true)
        .await
        .unwrap();
    world.sharing.sync(emma).await.unwrap();

    // The edit reaches the local replica, but the push fails.
    world
        .store
        .update_note_text(note.id, "Lost tooth", "The first one, at school.")
        .await
        .unwrap();
    world.sync.stop();
    assert!(matches!(
        world.sharing.sync(emma).await,
        Err(Error::Sync(_))
    ));

    world.sync.restart();
    world.sharing.sync(emma).await.unwrap();
    let view = world.child_view(&share.access_key).await.unwrap();
    assert_eq!(view.notes["Lost tooth"].body, "The first one, at school.");
}

#[tokio::test]
async fn stopping_deletes_the_cloud_copy_and_the_replica() {
    let Some(world) = World::new().await else {
        return;
    };
    let emma = world.child("Emma").await;
    let share = world.sharing.start_sharing(emma).await.unwrap();
    // Starting twice gives the same share, not a second database.
    let again = world.sharing.start_sharing(emma).await.unwrap();
    assert_eq!(again.share_id, share.share_id);
    world.sharing.sync(emma).await.unwrap();
    let name = share.database_url.rsplit('/').next().unwrap().to_owned();
    assert!(world.sync.dir.join(&name).exists());

    world.sharing.stop_sharing(emma).await.unwrap();

    assert!(world.store.share(emma).await.unwrap().is_none());
    assert!(replica_files(&world.shares_dir, emma).is_empty());
    assert!(
        !world.sync.dir.join(&name).exists(),
        "the cloud copy is gone"
    );
    assert_relay_refuses(world.child_view(&share.access_key).await);
    // Now Emma can be deleted.
    world.store.delete_child(emma).await.unwrap();
}

#[tokio::test]
async fn signing_out_keeps_shares_but_stops_syncing() {
    let Some(world) = World::new().await else {
        return;
    };
    let emma = world.child("Emma").await;
    world.sharing.start_sharing(emma).await.unwrap();

    world.sharing.log_out().await.unwrap();
    assert!(world.store.account().await.unwrap().is_none());
    assert!(matches!(
        world.sharing.sync(emma).await,
        Err(Error::NotSignedIn)
    ));
    assert!(matches!(
        world.sharing.start_sharing(world.child("Leo").await).await,
        Err(Error::NotSignedIn)
    ));

    world
        .sharing
        .log_in(
            world.relay_url.clone(),
            EMAIL.into(),
            Secret::new("correct horse battery"),
        )
        .await
        .unwrap();
    world.sharing.sync(emma).await.unwrap();

    let wrong = world
        .sharing
        .log_in(world.relay_url.clone(), EMAIL.into(), Secret::new("wrong"))
        .await;
    assert!(matches!(
        wrong,
        Err(Error::Relay {
            code: ErrorCode::InvalidCredentials,
            ..
        })
    ));
}
