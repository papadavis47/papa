//! What the end-to-end sharing tests share: Turso's sync server, an
//! in-process relay, and a father's vault signed in to it.
//!
//! Needs `tursodb` 0.8.2 on the `PATH`, or in `$TURSODB`. Without it the
//! tests say so and pass, unless `PAPA_REQUIRE_TURSODB` is set (CI sets it).

// Each test file uses its own share of these.
#![allow(dead_code)]

use std::net::TcpListener as StdListener;
use std::path::{Path, PathBuf};
use std::process::{Child as Process, Command, Stdio};
use std::time::{Duration, Instant};

use papa_core::relay::RelayClient;
use papa_core::{ChildDraft, ChildId, Sharing, Store};
use papa_proto::Secret;
use papa_relay::provision::LocalSync;
use papa_relay::rate_limit::RateLimiter;
use papa_relay::{AppState, Provisioner, Shared, now_ms, secrets};
use tempfile::TempDir;

pub const EMAIL: &str = "dad@example.com";
pub const DAY_MS: i64 = 24 * 60 * 60 * 1000;

/// `tursodb --sync-server` on a free port, serving databases from a folder.
pub struct SyncServer {
    pub process: Process,
    pub port: u16,
    pub dir: PathBuf,
    pub bin: PathBuf,
}

impl SyncServer {
    pub fn start(bin: PathBuf, dir: PathBuf, port: u16) -> Self {
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

    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    pub fn stop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }

    pub fn restart(&mut self) {
        self.stop();
        *self = Self::start(self.bin.clone(), self.dir.clone(), self.port);
    }
}

impl Drop for SyncServer {
    fn drop(&mut self) {
        self.stop();
    }
}

pub fn tursodb() -> Option<PathBuf> {
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

pub fn free_port() -> u16 {
    StdListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Everything a test needs: the sync server, a relay, a vault and the
/// father's side of sharing, signed in.
pub struct World {
    pub root: PathBuf,
    pub sync: SyncServer,
    pub relay_url: String,
    pub relay_state: AppState,
    pub store: Store,
    pub sharing: Sharing,
    pub shares_dir: PathBuf,
    /// Last, so it's deleted after the sync server has stopped.
    _dir: TempDir,
}

impl World {
    pub async fn new() -> Option<Self> {
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

    pub async fn invite(&self) -> String {
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

    pub async fn child(&self, name: &str) -> ChildId {
        let draft = ChildDraft {
            name: name.into(),
            color: 0xe0795b,
        };
        self.store.add_child(draft).await.unwrap().id
    }

    pub fn relay(&self) -> RelayClient {
        RelayClient::new(&self.relay_url).unwrap()
    }
}

pub fn sample_png(width: u32, height: u32) -> Vec<u8> {
    let pixels = image::RgbImage::from_fn(width, height, |x, y| image::Rgb([x as u8, y as u8, 7]));
    let mut bytes = std::io::Cursor::new(Vec::new());
    pixels
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
    bytes.into_inner()
}

pub fn replica_files(dir: &Path, child: ChildId) -> Vec<String> {
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
