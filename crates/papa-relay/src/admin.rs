//! Admin commands: minting and listing invite codes.
//!
//! Turso locks the relay's database file while the relay has it open, so
//! `papa-relay admin ...` can't open it alongside a running server. Instead
//! the server listens on a Unix socket next to its database, and the admin
//! command sends it one JSON line and reads one back. The socket is a file
//! only the relay's own user can open, so admin access never touches the
//! network and needs no password. When no relay is running, the command
//! opens the database itself.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::RelayError;
use crate::store::{InviteInfo, Store};
use crate::{now_ms, secrets};

const DAY_MS: i64 = 24 * 60 * 60 * 1000;

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum Request {
    CreateInvite { note: String, days: i64 },
    ListInvites,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "reply", rename_all = "snake_case")]
pub enum Reply {
    Invite { code: String, expires_at: i64 },
    Invites { invites: Vec<Invite> },
    Error { message: String },
}

/// [`InviteInfo`] on the wire.
#[derive(Debug, Serialize, Deserialize)]
pub struct Invite {
    pub note: String,
    pub created_at: i64,
    pub expires_at: i64,
    pub used_by_email: Option<String>,
}

impl From<InviteInfo> for Invite {
    fn from(info: InviteInfo) -> Self {
        Self {
            note: info.note,
            created_at: info.created_at,
            expires_at: info.expires_at,
            used_by_email: info.used_by_email,
        }
    }
}

/// Runs one admin request against the store.
pub async fn handle(store: &Store, request: Request) -> Reply {
    let result = match request {
        Request::CreateInvite { note, days } => create_invite(store, &note, days).await,
        Request::ListInvites => store.invites().await.map(|invites| Reply::Invites {
            invites: invites.into_iter().map(Invite::from).collect(),
        }),
    };
    result.unwrap_or_else(|error| Reply::Error {
        message: error.to_string(),
    })
}

async fn create_invite(store: &Store, note: &str, days: i64) -> Result<Reply, RelayError> {
    if !(1..=365).contains(&days) {
        return Err(RelayError::BadRequest("days must be 1 to 365".into()));
    }
    let code = secrets::new_invite_code()?;
    let hash = secrets::sha256(secrets::normalize_invite_code(&code).as_bytes());
    let now = now_ms();
    let expires_at = now + days * DAY_MS;
    store
        .add_invite(&hash, note.trim(), now, expires_at)
        .await?;
    tracing::info!(note, days, "invite created");
    Ok(Reply::Invite { code, expires_at })
}

/// The admin socket for a relay database: `relay.db` → `relay.db.admin.sock`.
pub fn socket_path(db_path: &Path) -> std::path::PathBuf {
    let mut name = db_path.as_os_str().to_owned();
    name.push(".admin.sock");
    name.into()
}

#[cfg(unix)]
pub use unix::{listen, send};

#[cfg(unix)]
mod unix {
    use std::os::unix::fs::PermissionsExt as _;
    use std::path::Path;

    use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader};
    use tokio::net::{UnixListener, UnixStream};

    use super::{Reply, Request, handle};
    use crate::AppState;

    /// Requests are one short line; anything longer is refused.
    const MAX_LINE: usize = 4096;

    /// Starts answering admin requests on `path`, in the background.
    pub fn listen(path: &Path, state: AppState) -> std::io::Result<()> {
        // A socket file left by a relay that didn't shut down cleanly would
        // make `bind` fail.
        match std::fs::remove_file(path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
        let listener = UnixListener::bind(path)?;
        // Only the relay's own user may connect.
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, _)) => {
                        tokio::spawn(answer(stream, state.clone()));
                    }
                    Err(error) => tracing::warn!(%error, "admin socket"),
                }
            }
        });
        Ok(())
    }

    async fn answer(stream: UnixStream, state: AppState) {
        let (read, mut write) = stream.into_split();
        let mut line = String::new();
        let mut reader = BufReader::new(read.take(MAX_LINE as u64));
        let reply = match reader.read_line(&mut line).await {
            Ok(_) => match serde_json::from_str::<Request>(&line) {
                Ok(request) => handle(&state.store, request).await,
                Err(e) => Reply::Error {
                    message: format!("bad request: {e}"),
                },
            },
            Err(e) => Reply::Error {
                message: e.to_string(),
            },
        };
        let mut json = serde_json::to_string(&reply).unwrap_or_default();
        json.push('\n');
        let _ = write.write_all(json.as_bytes()).await;
    }

    /// Sends `request` to the relay listening on `path`. `Ok(None)` when no
    /// relay is listening there.
    pub async fn send(path: &Path, request: &Request) -> std::io::Result<Option<Reply>> {
        let stream = match UnixStream::connect(path).await {
            Ok(stream) => stream,
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
                ) =>
            {
                return Ok(None);
            }
            Err(e) => return Err(e),
        };
        let (read, mut write) = stream.into_split();
        let mut json = serde_json::to_string(request).map_err(std::io::Error::other)?;
        json.push('\n');
        write.write_all(json.as_bytes()).await?;
        let mut line = String::new();
        BufReader::new(read).read_line(&mut line).await?;
        serde_json::from_str(&line)
            .map(Some)
            .map_err(std::io::Error::other)
    }
}
