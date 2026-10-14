//! Creating and deleting share databases, and minting tokens for them.
//!
//! Two backends, picked by `PAPA_RELAY_MODE`:
//!
//! - **Local**: a `tursodb --sync-server` on the developer's machine. It
//!   creates a database the first time a client syncs with
//!   `<url>/db/<name>`, and doesn't check tokens at all.
//! - **Cloud**: Turso Cloud, through its Platform API. Each share is a real
//!   database, and each token is a JWT scoped to that one database.
//!
//! An enum rather than a trait object: there are exactly two backends, a
//! `match` makes every operation handle both, and `async fn` works without
//! boxing futures.

use std::path::PathBuf;
use std::time::Duration;

use papa_proto::{Mode, Secret};
use serde::Deserialize;
use serde_json::json;

use crate::error::RelayError;

/// How long a database token lasts. Short, because the app can always ask
/// again, and a leaked token is soon useless.
pub const TOKEN_LIFETIME: Duration = Duration::from_secs(60 * 60);
/// [`TOKEN_LIFETIME`] as the Platform API takes it.
const TOKEN_EXPIRATION: &str = "1h";

/// Who a database token is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    /// Everything in that share database.
    Father,
    /// Read the notes; read and add messages and read receipts.
    Child,
}

pub enum Provisioner {
    Local(LocalSync),
    Cloud(TursoCloud),
}

impl Provisioner {
    pub fn mode(&self) -> Mode {
        match self {
            Self::Local(_) => Mode::Local,
            Self::Cloud(_) => Mode::Cloud,
        }
    }

    /// Creates the database `name` and returns the URL clients sync with.
    pub async fn create_database(&self, name: &str) -> Result<String, RelayError> {
        match self {
            Self::Local(local) => Ok(local.database_url(name)),
            Self::Cloud(cloud) => cloud.create_database(name).await,
        }
    }

    /// Deletes the database `name` and everything in it. Deleting one
    /// that's already gone succeeds, so a revoke can be retried.
    pub async fn delete_database(&self, name: &str) -> Result<(), RelayError> {
        match self {
            Self::Local(local) => local.delete_database(name),
            Self::Cloud(cloud) => cloud.delete_database(name).await,
        }
    }

    /// A token for the database `name` that lasts [`TOKEN_LIFETIME`].
    pub async fn mint_token(&self, name: &str, access: Access) -> Result<Secret, RelayError> {
        match self {
            Self::Local(_) => {
                // The local server ignores tokens. The app sends one anyway,
                // so its code is the same in both modes.
                let access = match access {
                    Access::Father => "father",
                    Access::Child => "child",
                };
                let random = crate::secrets::new_token()?;
                Ok(Secret::new(format!("local-{access}-{random}")))
            }
            Self::Cloud(cloud) => cloud.mint_token(name, access).await,
        }
    }
}

/// A local `tursodb --sync-server <addr> --sync-dir <dir>`.
pub struct LocalSync {
    base_url: String,
    /// The server's `--sync-dir`, if the relay may delete from it.
    sync_dir: Option<PathBuf>,
}

impl LocalSync {
    pub fn new(base_url: &str, sync_dir: Option<PathBuf>) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_owned(),
            sync_dir,
        }
    }

    fn database_url(&self, name: &str) -> String {
        format!("{}/db/{name}", self.base_url)
    }

    fn delete_database(&self, name: &str) -> Result<(), RelayError> {
        let Some(sync_dir) = &self.sync_dir else {
            tracing::warn!(
                name,
                "PAPA_RELAY_LOCAL_SYNC_DIR is unset; leaving the files"
            );
            return Ok(());
        };
        // Names are ours (`share-<hex>`), but a path built from a database
        // value is checked anyway before anything is deleted.
        if !is_database_name(name) {
            return Err(RelayError::Internal(format!("odd database name {name:?}")));
        }
        match std::fs::remove_dir_all(sync_dir.join(name)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(RelayError::Provisioning(format!("deleting {name}: {e}"))),
        }
    }
}

fn is_database_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Turso Cloud's Platform API, for one organization and group.
pub struct TursoCloud {
    http: reqwest::Client,
    /// `https://api.turso.tech/v1/organizations/<org>`
    org_url: String,
    group: String,
    token: Secret,
}

impl TursoCloud {
    pub fn new(api_url: &str, org: &str, group: &str, token: Secret) -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .expect("a client with only a timeout set always builds"),
            org_url: format!("{}/v1/organizations/{org}", api_url.trim_end_matches('/')),
            group: group.to_owned(),
            token,
        }
    }

    async fn create_database(&self, name: &str) -> Result<String, RelayError> {
        #[derive(Deserialize)]
        struct Created {
            database: CreatedDatabase,
        }
        #[derive(Deserialize)]
        struct CreatedDatabase {
            #[serde(rename = "Hostname")]
            hostname: String,
        }

        let response = self
            .http
            .post(format!("{}/databases", self.org_url))
            .bearer_auth(self.token.expose())
            .json(&json!({ "name": name, "group": self.group }))
            .send()
            .await
            .map_err(unreachable)?;
        let created: Created = check(response, "creating a database")
            .await?
            .json()
            .await
            .map_err(unreadable)?;
        Ok(format!("https://{}", created.database.hostname))
    }

    async fn delete_database(&self, name: &str) -> Result<(), RelayError> {
        let response = self
            .http
            .delete(format!("{}/databases/{name}", self.org_url))
            .bearer_auth(self.token.expose())
            .send()
            .await
            .map_err(unreachable)?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(());
        }
        check(response, "deleting a database").await?;
        Ok(())
    }

    async fn mint_token(&self, name: &str, access: Access) -> Result<Secret, RelayError> {
        #[derive(Deserialize)]
        struct Minted {
            jwt: String,
        }

        // Fine-grained permissions (`turso db tokens create -p`). The tables
        // are the share schema of PLAN.md §4, which Phase 11 creates. Turso's
        // API reference doesn't list this field yet; it's what the official
        // CLI sends.
        let body = match access {
            Access::Father => json!({}),
            Access::Child => json!({
                "fine_grained_permissions": [
                    { "t": ["shared_note", "shared_blob"], "a": ["data_read"] },
                    { "t": ["message", "receipt"], "a": ["data_read", "data_add"] },
                ]
            }),
        };
        let response = self
            .http
            .post(format!("{}/databases/{name}/auth/tokens", self.org_url))
            .query(&[
                ("expiration", TOKEN_EXPIRATION),
                ("authorization", "full-access"),
            ])
            .bearer_auth(self.token.expose())
            .json(&body)
            .send()
            .await
            .map_err(unreachable)?;
        let minted: Minted = check(response, "minting a token")
            .await?
            .json()
            .await
            .map_err(unreadable)?;
        Ok(Secret::new(minted.jwt))
    }
}

/// Turns a non-2xx reply into an error that names what was being done.
async fn check(response: reqwest::Response, doing: &str) -> Result<reqwest::Response, RelayError> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let body = response.text().await.unwrap_or_default();
    let body: String = body.chars().take(300).collect();
    Err(RelayError::Provisioning(format!(
        "{doing}: {status} {body}"
    )))
}

fn unreachable(error: reqwest::Error) -> RelayError {
    // reqwest's message can include the URL, which holds no secrets (the
    // token goes in a header).
    RelayError::Provisioning(format!("couldn't reach the Platform API: {error}"))
}

fn unreadable(error: reqwest::Error) -> RelayError {
    RelayError::Provisioning(format!("unexpected reply from the Platform API: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_urls_and_deletes_stay_inside_the_sync_dir() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path();
        std::fs::create_dir(dir.join("share-ab12")).unwrap();
        let local = LocalSync::new("http://127.0.0.1:8080/", Some(dir.to_owned()));

        assert_eq!(
            local.database_url("share-ab12"),
            "http://127.0.0.1:8080/db/share-ab12"
        );
        assert!(local.delete_database("../escape").is_err());
        local.delete_database("share-ab12").unwrap();
        assert!(!dir.join("share-ab12").exists());
        // Already gone: still fine.
        local.delete_database("share-ab12").unwrap();
    }
}
