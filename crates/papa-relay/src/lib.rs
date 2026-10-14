//! Papa's relay server: father accounts, invite codes, one cloud database
//! per shared child, and short-lived tokens scoped to one database
//! (PLAN.md §2.2, ADR 0005, ADR 0014).
//!
//! It never sees plaintext: everything in a share database is encrypted on
//! the father's or child's device (Phase 11). The relay only decides who
//! may reach which database.
//!
//! A library as well as a binary, so the integration tests can start a
//! relay in-process.

use std::net::SocketAddr;
use std::ops::Deref;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::net::TcpListener;

pub mod admin;
pub mod api;
pub mod config;
pub mod error;
pub mod provision;
pub mod rate_limit;
pub mod secrets;
pub mod store;

pub use config::Config;
pub use error::RelayError;
pub use provision::Provisioner;
pub use store::Store;

/// What every request handler can reach. Cheap to clone (one `Arc`), which
/// axum does for each request.
#[derive(Clone)]
pub struct AppState(Arc<Shared>);

pub struct Shared {
    pub store: Store,
    pub provisioner: Provisioner,
    pub rate_limiter: rate_limit::RateLimiter,
    /// See [`Config::client_ip_header`].
    pub client_ip_header: Option<String>,
}

impl AppState {
    pub fn new(shared: Shared) -> Self {
        Self(Arc::new(shared))
    }
}

/// Lets handlers write `state.store` instead of `state.0.store`.
impl Deref for AppState {
    type Target = Shared;

    fn deref(&self) -> &Shared {
        &self.0
    }
}

/// Serves the API on `listener` until `shutdown` completes.
pub async fn serve(
    listener: TcpListener,
    state: AppState,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    // `with_connect_info` hands each request the peer's address, which the
    // rate limiter needs.
    let app = api::router(state).into_make_service_with_connect_info::<SocketAddr>();
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await
}

/// Now, in Unix milliseconds.
pub fn now_ms() -> i64 {
    let since_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    since_epoch.as_millis() as i64
}
