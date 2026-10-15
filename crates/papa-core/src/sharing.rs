//! Sharing, the father's side: his relay account, starting and stopping
//! sharing with a child, new access keys, and syncing shared notes to each
//! child's cloud database.
//!
//! Like [`Store`], it's an actor on a thread of its own, here called
//! "papa-sharing". It needs one: the relay client and Turso's sync both run
//! on tokio's IO, and a sync can wait on the network for a while, which the
//! vault's thread mustn't. It talks to the vault through a [`Store`] clone,
//! like the UI does.
//!
//! ```text
//!  UI ──▶ Sharing ──┬─▶ Store (vault): keys, the projection, sealing
//!                   ├─▶ RelayClient: accounts, shares, tokens
//!                   └─▶ Replica (shares/<child>.db) ──push──▶ cloud
//! ```
//!
//! A sync works out what the child's database is missing, writes it to the
//! local replica, and pushes. The vault is the truth, and syncing again is
//! always safe: see [`projection`](crate::share::projection).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;

use jiff::Timestamp;
use papa_proto::{ErrorCode, Secret};
use tokio::sync::{mpsc, oneshot};

use crate::relay::RelayClient;
use crate::share::replica::{self, Replica, TokenCell};
use crate::share::{AccessKey, ChildKeys, ContentKey};
use crate::store::Rekey;
use crate::{Account, ChildId, ChildShare, Error, Result, Store};

/// How many bytes of sealed images go into the replica before a push.
/// Bounds memory on a first sync of many photos, and saves progress: an
/// interrupted sync resumes after the last batch that was pushed.
const IMAGE_BATCH_BYTES: usize = 16 * 1024 * 1024;

/// A database token is renewed this long before it expires, so it can't
/// run out halfway through a push.
const TOKEN_MARGIN_MS: i64 = 5 * 60 * 1000;

/// What a sync sent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SyncReport {
    pub notes: usize,
    pub removed_notes: usize,
    pub images: usize,
    pub removed_images: usize,
}

impl SyncReport {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// A handle to the sharing thread. Cheap to clone. The thread stops when
/// the last clone is dropped.
#[derive(Clone, Debug)]
pub struct Sharing {
    requests: mpsc::UnboundedSender<Request>,
}

type Reply<T> = oneshot::Sender<Result<T>>;

enum Request {
    SignUp {
        relay_url: String,
        invite_code: Secret,
        email: String,
        password: Secret,
        reply: Reply<Account>,
    },
    LogIn {
        relay_url: String,
        email: String,
        password: Secret,
        reply: Reply<Account>,
    },
    LogOut(Reply<()>),
    Start(ChildId, Reply<ChildShare>),
    NewKey(ChildId, Reply<ChildShare>),
    Stop(ChildId, Reply<()>),
    Sync(ChildId, Reply<SyncReport>),
    SyncAll(Reply<Vec<(ChildId, Result<SyncReport>)>>),
}

impl Sharing {
    /// Starts the sharing thread. Replicas go in `shares_dir`
    /// ([`paths::shares_dir`](crate::paths::shares_dir)).
    pub fn start(store: Store, shares_dir: PathBuf) -> Result<Self> {
        let (requests, inbox) = mpsc::unbounded_channel();
        let worker = Worker {
            store,
            shares_dir,
            http: None,
            replicas: HashMap::new(),
        };
        thread::Builder::new()
            .name("papa-sharing".into())
            .spawn(move || worker.run(inbox))
            .map_err(Error::StartThread)?;
        Ok(Self { requests })
    }

    /// Creates a father account on the relay at `relay_url`, with an
    /// invite code, and signs in.
    pub async fn sign_up(
        &self,
        relay_url: String,
        invite_code: Secret,
        email: String,
        password: Secret,
    ) -> Result<Account> {
        self.call(|reply| Request::SignUp {
            relay_url,
            invite_code,
            email,
            password,
            reply,
        })
        .await
    }

    pub async fn log_in(
        &self,
        relay_url: String,
        email: String,
        password: Secret,
    ) -> Result<Account> {
        self.call(|reply| Request::LogIn {
            relay_url,
            email,
            password,
            reply,
        })
        .await
    }

    /// Signs out, here and at the relay. Shares stay; signing in again
    /// picks them up.
    pub async fn log_out(&self) -> Result<()> {
        self.call(Request::LogOut).await
    }

    /// Starts sharing with a child: a new access key, and a database for
    /// them at the relay. Nothing is shared yet; that's per note.
    pub async fn start_sharing(&self, child: ChildId) -> Result<ChildShare> {
        self.call(|reply| Request::Start(child, reply)).await
    }

    /// Replaces a child's access key. The old one stops working, and
    /// everything shared is sealed again with a new content key at the
    /// next sync.
    pub async fn new_key(&self, child: ChildId) -> Result<ChildShare> {
        self.call(|reply| Request::NewKey(child, reply)).await
    }

    /// Stops sharing with a child: deletes their cloud database, the local
    /// replica, and the record of which notes they could see.
    pub async fn stop_sharing(&self, child: ChildId) -> Result<()> {
        self.call(|reply| Request::Stop(child, reply)).await
    }

    /// Brings one child's cloud database in line with the vault.
    pub async fn sync(&self, child: ChildId) -> Result<SyncReport> {
        self.call(|reply| Request::Sync(child, reply)).await
    }

    /// Syncs every share, one after the other. One failing doesn't stop
    /// the others.
    pub async fn sync_all(&self) -> Result<Vec<(ChildId, Result<SyncReport>)>> {
        self.call(Request::SyncAll).await
    }

    async fn call<T>(&self, request: impl FnOnce(Reply<T>) -> Request) -> Result<T> {
        let (reply, response) = oneshot::channel();
        self.requests
            .send(request(reply))
            .map_err(|_| Error::StoreClosed)?;
        response.await.map_err(|_| Error::StoreClosed)?
    }
}

/// An open replica, and the token it syncs with.
struct OpenReplica {
    replica: Replica,
    token: TokenCell,
    /// When `token` expires, in Unix milliseconds.
    expires_at: i64,
}

/// The sharing thread's state. Only that thread touches it, so it needs no
/// locks (the token cells are shared with Turso's IO thread, hence theirs).
struct Worker {
    store: Store,
    shares_dir: PathBuf,
    /// Made on first use: building one loads the system's certificates.
    http: Option<reqwest::Client>,
    replicas: HashMap<ChildId, OpenReplica>,
}

impl Worker {
    fn run(mut self, mut inbox: mpsc::UnboundedReceiver<Request>) {
        // `enable_all`: unlike the vault's thread, this one does network IO.
        let runtime = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            // Requests get `StoreClosed` when the inbox drops.
            Err(_) => return,
        };
        runtime.block_on(async move {
            while let Some(request) = inbox.recv().await {
                self.handle(request).await;
            }
        });
    }

    async fn handle(&mut self, request: Request) {
        // A failed `send` means the caller stopped waiting; the work is
        // done either way.
        match request {
            Request::SignUp {
                relay_url,
                invite_code,
                email,
                password,
                reply,
            } => {
                let result = self
                    .sign_in(relay_url, email, async |relay, email| {
                        relay.sign_up(invite_code, email, password).await
                    })
                    .await;
                let _ = reply.send(result);
            }
            Request::LogIn {
                relay_url,
                email,
                password,
                reply,
            } => {
                let result = self
                    .sign_in(relay_url, email, async |relay, email| {
                        relay.log_in(email, password).await
                    })
                    .await;
                let _ = reply.send(result);
            }
            Request::LogOut(reply) => {
                let _ = reply.send(self.log_out().await);
            }
            Request::Start(child, reply) => {
                let _ = reply.send(self.start_sharing(child).await);
            }
            Request::NewKey(child, reply) => {
                let _ = reply.send(self.new_key(child).await);
            }
            Request::Stop(child, reply) => {
                let _ = reply.send(self.stop_sharing(child).await);
            }
            Request::Sync(child, reply) => {
                let _ = reply.send(self.sync(child).await);
            }
            Request::SyncAll(reply) => {
                let _ = reply.send(self.sync_all().await);
            }
        }
    }

    fn relay(&mut self, relay_url: &str) -> Result<RelayClient> {
        let http = match &self.http {
            Some(http) => http.clone(),
            None => self.http.insert(crate::relay::http_client()?).clone(),
        };
        Ok(RelayClient::with_client(http, relay_url))
    }

    // --- The account ---

    /// Signs in with `call` (sign up or log in), and remembers the session.
    async fn sign_in(
        &mut self,
        relay_url: String,
        email: String,
        call: impl AsyncFnOnce(&RelayClient, &str) -> Result<papa_proto::Session>,
    ) -> Result<Account> {
        let relay_url = crate::relay::clean_url(&relay_url)?;
        let email = email.trim().to_owned();
        let relay = self.relay(&relay_url)?;
        let session = call(&relay, &email).await?;
        let account = Account {
            relay_url,
            email,
            session: session.token,
            expires_at: Timestamp::from_millisecond(session.expires_at).unwrap_or(Timestamp::MAX),
        };
        self.store.set_account(account.clone()).await?;
        // Tokens of another account's shares are no good now.
        self.replicas.clear();
        Ok(account)
    }

    async fn log_out(&mut self) -> Result<()> {
        if let Some(account) = self.store.account().await? {
            // Ending the session at the relay is a courtesy: forgetting the
            // token here is what signs out. Don't let a network problem
            // keep the father signed in.
            if let Ok(relay) = self.relay(&account.relay_url) {
                let _ = relay.log_out(&account.session).await;
            }
        }
        self.replicas.clear();
        self.store.clear_account().await
    }

    /// The account, if it's signed in and its session hasn't expired.
    async fn account(&self) -> Result<Account> {
        let account = self.store.account().await?.ok_or(Error::NotSignedIn)?;
        if account.expires_at <= Timestamp::now() {
            self.store.clear_account().await?;
            return Err(Error::NotSignedIn);
        }
        Ok(account)
    }

    /// The account, checked to be on the relay `share` was made on.
    async fn account_for(&self, share: &ChildShare) -> Result<Account> {
        let account = self.account().await?;
        if account.relay_url != share.relay_url {
            return Err(Error::WrongRelay(share.relay_url.clone()));
        }
        Ok(account)
    }

    /// Passes `result` on. If the relay said the session is no good,
    /// forgets it first, so the app shows the father signed out.
    async fn session_check<T>(&mut self, result: Result<T>) -> Result<T> {
        if let Err(Error::Relay {
            code: ErrorCode::Unauthorized,
            ..
        }) = &result
        {
            self.replicas.clear();
            self.store.clear_account().await?;
        }
        result
    }

    // --- Shares ---

    async fn share(&self, child: ChildId) -> Result<ChildShare> {
        self.store
            .share(child)
            .await?
            .ok_or(Error::NotFound("share"))
    }

    async fn start_sharing(&mut self, child: ChildId) -> Result<ChildShare> {
        // Asking twice is harmless: the share that exists is the answer.
        if let Some(share) = self.store.share(child).await? {
            return Ok(share);
        }
        let account = self.account().await?;
        // Make sure the child exists before provisioning anything for them.
        if !self.store.children().await?.iter().any(|c| c.id == child) {
            return Err(Error::NotFound("child"));
        }
        let access_key = AccessKey::generate()?;
        let keys = ChildKeys::derive(&access_key)?;
        let relay = self.relay(&account.relay_url)?;
        let created = relay.create_share(&account.session, &keys.auth).await;
        let created = self.session_check(created).await?;

        let epoch = 1;
        let content_key = ContentKey::generate()?;
        let share = ChildShare {
            child_id: child,
            wrapped_key: content_key.wrap(&keys.enc, &created.id, epoch)?,
            share_id: created.id,
            relay_url: account.relay_url.clone(),
            database_url: created.database_url,
            access_key,
            key_epoch: epoch,
            auth_key_stale: false,
            created_at: Timestamp::now(),
            last_synced_at: None,
        };
        if let Err(err) = self.store.add_share(share.clone(), content_key).await {
            // Don't leave a database at the relay that nothing here knows.
            let _ = relay.delete_share(&account.session, &share.share_id).await;
            return Err(err);
        }
        Ok(share)
    }

    async fn new_key(&mut self, child: ChildId) -> Result<ChildShare> {
        let share = self.share(child).await?;
        let account = self.account_for(&share).await?;
        let access_key = AccessKey::generate()?;
        let keys = ChildKeys::derive(&access_key)?;
        let content_key = ContentKey::generate()?;
        let epoch = share.key_epoch + 1;
        let wrapped = content_key.wrap(&keys.enc, &share.share_id, epoch)?;
        // The vault first: once the new key is stored, nothing can lose it.
        // The relay hears of it next; if that fails, the share stays marked
        // stale and the next sync tells the relay.
        let share = self
            .store
            .rekey_share(Rekey {
                child,
                epoch,
                access_key,
                content_key,
                wrapped,
            })
            .await?;
        self.send_auth_key(&account, &share, &keys).await?;
        self.share(child).await
    }

    /// Tells the relay a share's current access key.
    async fn send_auth_key(
        &mut self,
        account: &Account,
        share: &ChildShare,
        keys: &ChildKeys,
    ) -> Result<()> {
        let relay = self.relay(&account.relay_url)?;
        let sent = relay
            .set_auth_key(&account.session, &share.share_id, &keys.auth)
            .await;
        self.session_check(sent).await?;
        self.store
            .auth_key_updated(share.child_id, share.key_epoch)
            .await
    }

    async fn stop_sharing(&mut self, child: ChildId) -> Result<()> {
        let share = self.share(child).await?;
        let account = self.account_for(&share).await?;
        let relay = self.relay(&account.relay_url)?;
        // The cloud copy first: if deleting it fails, the share stays
        // here, so the father can try again.
        let deleted = relay.delete_share(&account.session, &share.share_id).await;
        self.session_check(deleted).await?;
        // Dropping the replica closes it before its files go.
        self.replicas.remove(&child);
        replica::remove_files(&self.replica_path(child))?;
        self.store.remove_share(child).await
    }

    // --- Syncing ---

    async fn sync_all(&mut self) -> Result<Vec<(ChildId, Result<SyncReport>)>> {
        let mut results = Vec::new();
        for share in self.store.shares().await? {
            let result = self.sync(share.child_id).await;
            results.push((share.child_id, result));
        }
        Ok(results)
    }

    async fn sync(&mut self, child: ChildId) -> Result<SyncReport> {
        let share = self.share(child).await?;
        let account = self.account_for(&share).await?;
        if share.auth_key_stale {
            let keys = ChildKeys::derive(&share.access_key)?;
            self.send_auth_key(&account, &share, &keys).await?;
        }
        let result = self.push_changes(&account, &share).await;
        if result.is_err() {
            // Reopen next time, in case the replica is what's broken.
            self.replicas.remove(&child);
        }
        let report = result?;
        self.store.share_synced(child, Timestamp::now()).await?;
        Ok(report)
    }

    async fn push_changes(&mut self, account: &Account, share: &ChildShare) -> Result<SyncReport> {
        let child = share.child_id;
        self.open_replica(account, share).await?;
        let store = self.store.clone();
        let replica = &mut self
            .replicas
            .get_mut(&child)
            .expect("opened just above")
            .replica;

        replica
            .set_share_key(share.key_epoch, &share.wrapped_key)
            .await?;
        let mut report = SyncReport::default();
        loop {
            let manifest = replica.manifest().await?;
            let changes = store
                .share_changes(child, manifest, IMAGE_BATCH_BYTES)
                .await?;
            replica.apply(&changes).await?;
            report.notes += changes.notes.len();
            report.removed_notes += changes.removed_notes.len();
            report.images += changes.blobs.len();
            report.removed_images += changes.removed_blobs.len();
            // Push every batch, and push even when nothing changed: the
            // manifest describes the local replica, which is ahead of the
            // cloud if the last push failed. Turso remembers what it hasn't
            // sent yet, and sends only that.
            replica.push().await?;
            if !changes.more {
                return Ok(report);
            }
        }
    }

    /// Opens `share`'s replica if it isn't open, and makes sure its token
    /// is good for a while yet.
    async fn open_replica(&mut self, account: &Account, share: &ChildShare) -> Result<()> {
        let child = share.child_id;
        let now = Timestamp::now().as_millisecond();
        if self
            .replicas
            .get(&child)
            .is_some_and(|open| open.expires_at - TOKEN_MARGIN_MS > now)
        {
            return Ok(());
        }
        let relay = self.relay(&account.relay_url)?;
        let token = relay.father_token(&account.session, &share.share_id).await;
        let token = self.session_check(token).await?;

        if let Some(open) = self.replicas.get_mut(&child) {
            *open.token.lock().expect("never poisoned") = token.token;
            open.expires_at = token.expires_at;
            return Ok(());
        }
        crate::files::create_private_dir(&self.shares_dir)?;
        let cell: TokenCell = Arc::new(Mutex::new(token.token));
        let replica =
            Replica::open(&self.replica_path(child), &token.database_url, cell.clone()).await?;
        self.replicas.insert(
            child,
            OpenReplica {
                replica,
                token: cell,
                expires_at: token.expires_at,
            },
        );
        Ok(())
    }

    fn replica_path(&self, child: ChildId) -> PathBuf {
        self.shares_dir.join(format!("{child}.db"))
    }
}
